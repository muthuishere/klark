//! `klark integration hook <agent> [action]`: the Rust half of the Windows
//! `.cmd` hooks.
//!
//! An agent runs klark's `.cmd` hook with its event as JSON on stdin; the
//! script hands everything to this command. Each agent's rules are a direct
//! port of the PowerShell hooks they replace (guards, event filters, field
//! fallbacks and `seq` units), and the report goes to the server through
//! `klark pane report-agent-session` / `klark pane report-agent`, as before.
//!
//! The command always exits 0 and never writes to stderr. Cursor and
//! Antigravity CLI read a JSON object from stdout, so for them it prints
//! exactly one `{}`, last; every other agent gets no output.
//!
//! PowerShell semantics kept on purpose: `-eq`/`-ne`/`-in` comparisons are
//! case-insensitive (Claude's `-cne` stays case-sensitive), and a field the
//! ps1 read without an `-is [string]` check also accepts a number or boolean,
//! stringified as PowerShell would.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// `[DateTime]::UtcNow.Ticks` is 100 ns units since 0001-01-01; this is the
/// tick count of the Unix epoch.
const DOTNET_TICKS_AT_UNIX_EPOCH: u128 = 621_355_968_000_000_000;
/// Letta injects hook output into the next prompt and waits for the hook, so
/// the report is capped like the ps1's `Wait-Job -Timeout 1`.
const LETTA_REPORT_TIMEOUT: Duration = Duration::from_secs(1);
/// `devin list` fallback cap, as in the bash hook (the ps1 had none).
const DEVIN_LIST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookAgent {
    AntigravityCli,
    Claude,
    Codex,
    Copilot,
    Cursor,
    Devin,
    Droid,
    Grok,
    Kimi,
    Letta,
    Mastracode,
    Qodercli,
    Qwen,
}

impl HookAgent {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "antigravity_cli" | "antigravity-cli" => Self::AntigravityCli,
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "copilot" => Self::Copilot,
            "cursor" => Self::Cursor,
            "devin" => Self::Devin,
            "droid" => Self::Droid,
            "grok" => Self::Grok,
            "kimi" => Self::Kimi,
            "letta" => Self::Letta,
            "mastracode" => Self::Mastracode,
            "qodercli" => Self::Qodercli,
            "qwen" => Self::Qwen,
            _ => return None,
        })
    }

    /// The agent reads a JSON object from the hook's stdout.
    pub(crate) fn prints_empty_json(self) -> bool {
        matches!(self, Self::Cursor | Self::AntigravityCli)
    }

    fn report_timeout(self) -> Option<Duration> {
        (self == Self::Letta).then_some(LETTA_REPORT_TIMEOUT)
    }
}

/// What the hook may read besides stdin. Injected so the rules are testable
/// without touching the process environment or spawning anything.
pub(crate) struct HookContext<'a> {
    pub(crate) env: &'a dyn Fn(&str) -> Option<String>,
    /// Nanoseconds since the Unix epoch, for `seq`.
    pub(crate) unix_nanos: u128,
    /// `(Get-Location).Path`, for the Devin fallback.
    pub(crate) cwd: &'a dyn Fn() -> Option<PathBuf>,
    /// stdout of `devin list --format json`, for the Devin fallback.
    pub(crate) devin_list: &'a dyn Fn() -> Option<String>,
}

impl HookContext<'_> {
    fn env_text(&self, name: &str) -> Option<String> {
        (self.env)(name).filter(|value| !is_blank(value))
    }

    /// `[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()`
    fn unix_millis(&self) -> String {
        (self.unix_nanos / 1_000_000).to_string()
    }

    /// `[DateTime]::UtcNow.Ticks` / `[DateTimeOffset]::UtcNow.Ticks`
    fn dotnet_ticks(&self) -> String {
        (self.unix_nanos / 100 + DOTNET_TICKS_AT_UNIX_EPOCH).to_string()
    }

    /// `$env:KLARK_ENV -ne "1"`, a blank `KLARK_PANE_ID` and, for some
    /// agents, a blank `KLARK_SOCKET_PATH` end the hook. Returns the pane id.
    fn klark_pane(&self, needs_socket: bool) -> Option<String> {
        if (self.env)("KLARK_ENV").as_deref() != Some("1") {
            return None;
        }
        let pane = self.env_text("KLARK_PANE_ID")?;
        if needs_socket && self.env_text("KLARK_SOCKET_PATH").is_none() {
            return None;
        }
        Some(pane)
    }
}

/// The hook's stdin after `ConvertFrom-Json`.
enum Payload {
    /// Blank input (`$payload = $null`).
    Empty,
    /// Not JSON (the ps1's `catch`).
    Invalid,
    Json(Value),
}

impl Payload {
    fn parse(text: &str) -> Self {
        if is_blank(text) {
            return Self::Empty;
        }
        match serde_json::from_str(text) {
            Ok(value) => Self::Json(value),
            Err(_) => Self::Invalid,
        }
    }

    /// The payload's fields, where the ps1 treats a failed parse as `$null`.
    /// A JSON value that is not an object has no fields.
    fn fields(&self) -> Option<&Map<String, Value>> {
        match self {
            Self::Json(value) => value.as_object(),
            Self::Empty | Self::Invalid => None,
        }
    }
}

/// The `klark` arguments that report this hook event, or `None` when the ps1
/// would have exited without reporting.
pub(crate) fn hook_report_args(
    agent: HookAgent,
    action: &str,
    payload_text: &str,
    ctx: &HookContext<'_>,
) -> Option<Vec<String>> {
    // `StreamReader` drops a UTF-8 byte order mark.
    let payload_text = payload_text
        .strip_prefix('\u{feff}')
        .unwrap_or(payload_text);
    match agent {
        HookAgent::AntigravityCli => antigravity_cli(action, payload_text, ctx),
        HookAgent::Claude => claude(action, payload_text, ctx),
        HookAgent::Codex => codex(action, payload_text, ctx),
        HookAgent::Copilot => copilot(payload_text, ctx),
        HookAgent::Cursor => cursor(action, payload_text, ctx),
        HookAgent::Devin => devin(action, payload_text, ctx),
        HookAgent::Droid => session_id_only(action, payload_text, ctx, "droid", false),
        HookAgent::Grok => grok(action, payload_text, ctx),
        HookAgent::Kimi => kimi(action, payload_text, ctx),
        HookAgent::Letta => letta(action, payload_text, ctx),
        HookAgent::Mastracode => mastracode(action, payload_text, ctx),
        HookAgent::Qodercli => session_id_only(action, payload_text, ctx, "qodercli", false),
        HookAgent::Qwen => qwen(action, payload_text, ctx),
    }
}

fn claude(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    if matches!(payload, Payload::Invalid) {
        return None;
    }
    if (ctx.env)("CURSOR_VERSION").is_some() {
        return None;
    }
    let fields = payload.fields()?;
    if fields.contains_key("cursor_version") {
        return None;
    }
    // `-cne`: the only case-sensitive event check.
    if string_field(fields, "hook_event_name") != Some("SessionStart") {
        return None;
    }
    if text_field(fields, "agent_id").is_some() {
        return None;
    }
    let session_id = text_field(fields, "session_id")?;

    let mut args = session_report(&pane, "klark:claude", "claude");
    push_pairs(
        &mut args,
        &[
            ("--seq", ctx.unix_millis()),
            ("--agent-session-id", session_id),
        ],
    );
    if let Some(path) = nonblank_string_field(fields, "transcript_path") {
        push_pairs(&mut args, &[("--agent-session-path", path)]);
    }
    if let Some(source) = nonblank_string_field(fields, "source") {
        push_pairs(&mut args, &[("--session-start-source", source)]);
    }
    Some(args)
}

fn codex(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    if matches!(payload, Payload::Invalid) {
        return None;
    }
    let fields = payload.fields()?;
    let event = fields.get("hook_event_name");
    let is_session_start = ps_text(event).is_some_and(|event| ieq(&event, "SessionStart"));
    if is_truthy(event) && !is_session_start {
        return None;
    }
    let session_id = text_field(fields, "session_id")?;
    text_field(fields, "transcript_path")?;
    if let Some(thread_id) = ctx.env_text("CODEX_THREAD_ID") {
        if !ieq(&thread_id, &session_id) {
            return None;
        }
    }

    let mut args = session_report(&pane, "klark:codex", "codex");
    push_pairs(
        &mut args,
        &[
            ("--seq", ctx.dotnet_ticks()),
            ("--agent-session-id", session_id),
        ],
    );
    if is_session_start {
        if let Some(source) = nonblank_string_field(fields, "source") {
            push_pairs(&mut args, &[("--session-start-source", source)]);
        }
    }
    Some(args)
}

/// Copilot's hook takes no action argument.
fn copilot(text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    let pane = ctx.klark_pane(true)?;
    // A blank or unparsable payload is `@{}`: no fields, then no session.
    let payload = Payload::parse(text);
    let empty = Map::new();
    let fields = payload.fields().unwrap_or(&empty);
    let first_text = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| nonblank_string_field(fields, name))
    };

    if let Some(event) = first_text(&["hook_event_name", "hookEventName"]) {
        let normalized = event.replace(['_', '-'], "").to_lowercase();
        if normalized != "sessionstart" {
            return None;
        }
    } else if fields.keys().any(|key| ieq(key, "prompt"))
        || first_text(&[
            "tool_name",
            "toolName",
            "notification_type",
            "notificationType",
            "stop_reason",
            "stopReason",
            "reason",
        ])
        .is_some()
    {
        return None;
    }

    let session_id =
        text_field(fields, "session_id").or_else(|| text_field(fields, "sessionId"))?;
    let mut args = session_report(&pane, "klark:copilot", "copilot");
    push_pairs(
        &mut args,
        &[
            ("--agent-session-id", session_id),
            ("--seq", ctx.unix_millis()),
        ],
    );
    Some(args)
}

fn cursor(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    // Cursor may print text before the JSON object.
    let text = text.find('{').map_or(text, |start| &text[start..]);
    let fields = match Payload::parse(text) {
        Payload::Json(Value::Object(fields)) => fields,
        _ => return None,
    };
    let event = match fields.get("hook_event_name") {
        Some(Value::String(event)) => Some(event.clone()),
        _ => ps_text(fields.get("hookEventName")),
    };
    if let Some(event) = event.filter(|event| !is_blank(event)) {
        if !ieq(&event, "sessionStart") {
            return None;
        }
    }
    let session_id = [
        "session_id",
        "sessionId",
        "conversation_id",
        "conversationId",
    ]
    .iter()
    .find_map(|name| nonblank_string_field(&fields, name))?;

    let mut args = session_report(&pane, "klark:cursor", "cursor");
    push_pairs(
        &mut args,
        &[
            ("--seq", ctx.dotnet_ticks()),
            ("--agent-session-id", session_id),
        ],
    );
    Some(args)
}

fn devin(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    let fields = payload.fields();

    // `elseif`: a string `session_id`, even a blank one, hides `sessionId`.
    let mut session_id = fields.and_then(|fields| {
        string_field(fields, "session_id")
            .or_else(|| string_field(fields, "sessionId"))
            .map(str::to_string)
    });
    let event = fields
        .and_then(|fields| string_field(fields, "hook_event_name"))
        .unwrap_or("");
    let startup = fields
        .and_then(|fields| ps_text(fields.get("source")))
        .is_some_and(|source| ieq(&source, "startup"));
    let allow_fallback =
        !(ieq(event, "UserPromptSubmit") || (ieq(event, "SessionStart") && startup));
    if session_id.as_deref().is_none_or(is_blank) && allow_fallback {
        if let Some(found) = devin_session_from_list(ctx) {
            session_id = Some(found);
        }
    }
    let session_id = session_id.filter(|id| !is_blank(id))?;

    let mut args = session_report(&pane, "klark:devin", "devin");
    push_pairs(
        &mut args,
        &[
            ("--seq", ctx.dotnet_ticks()),
            ("--agent-session-id", session_id),
        ],
    );
    Some(args)
}

/// The id of the `devin list` session whose working directory is the project
/// (`DEVIN_PROJECT_DIR`, else the current directory), compared as full paths
/// without a trailing `\`, ignoring case.
fn devin_session_from_list(ctx: &HookContext<'_>) -> Option<String> {
    let project = match ctx.env_text("DEVIN_PROJECT_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => (ctx.cwd)()?,
    };
    let sessions: Value = serde_json::from_str(&(ctx.devin_list)()?).ok()?;
    let project = full_path_key(&project)?;
    let sessions = match sessions {
        Value::Array(sessions) => sessions,
        other => vec![other],
    };
    for session in sessions {
        let (Some(Value::String(id)), Some(Value::String(dir))) =
            (session.get("id"), session.get("working_directory"))
        else {
            continue;
        };
        // `GetFullPath` throwing ends the whole lookup, as in the ps1's `try`.
        if full_path_key(Path::new(dir))? == project {
            return Some(id.clone());
        }
    }
    None
}

/// `[IO.Path]::GetFullPath($p).TrimEnd('\')`, lowercased for `-ieq`.
fn full_path_key(path: &Path) -> Option<String> {
    let full = std::path::absolute(path).ok()?;
    Some(full.to_string_lossy().trim_end_matches('\\').to_lowercase())
}

/// Droid and Qodercli: report the payload's `session_id`, nothing else.
fn session_id_only(
    action: &str,
    text: &str,
    ctx: &HookContext<'_>,
    agent: &str,
    needs_socket: bool,
) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(needs_socket)?;
    let payload = Payload::parse(text);
    let session_id = text_field(payload.fields()?, "session_id")?;

    let mut args = session_report(&pane, &format!("klark:{agent}"), agent);
    push_pairs(
        &mut args,
        &[
            ("--agent-session-id", session_id),
            ("--seq", ctx.unix_millis()),
        ],
    );
    Some(args)
}

fn grok(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    let fields = payload.fields();
    let event = fields.and_then(|fields| {
        string_field(fields, "hook_event_name").or_else(|| string_field(fields, "hookEventName"))
    });
    if let Some(event) = event {
        if !["session_start", "SessionStart", "sessionStart"]
            .iter()
            .any(|allowed| ieq(event, allowed))
        {
            return None;
        }
    }
    let session_start_source = fields.and_then(|fields| string_field(fields, "source"));

    let session_id = ctx.env_text("GROK_SESSION_ID").or_else(|| {
        fields.and_then(|fields| {
            string_field(fields, "session_id")
                .or_else(|| string_field(fields, "sessionId"))
                .map(str::to_string)
        })
    });
    let session_id = session_id.filter(|id| !is_blank(id))?;

    let mut args = session_report(&pane, "klark:grok", "grok");
    push_pairs(
        &mut args,
        &[
            ("--seq", ctx.dotnet_ticks()),
            ("--agent-session-id", session_id),
        ],
    );
    if let Some(source) = session_start_source.filter(|source| !is_blank(source)) {
        push_pairs(&mut args, &[("--session-start-source", source.to_string())]);
    }
    Some(args)
}

fn kimi(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !["session", "working", "blocked", "idle"]
        .iter()
        .any(|allowed| ieq(action, allowed))
    {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    let seq = ctx.unix_millis();
    let session_id = payload
        .fields()
        .and_then(|fields| text_field(fields, "session_id"));

    if ieq(action, "session") {
        let mut args = session_report(&pane, "klark:kimi", "kimi");
        push_pairs(
            &mut args,
            &[
                ("--agent-session-id", session_id?),
                ("--session-start-source", "startup".to_string()),
                ("--seq", seq),
            ],
        );
        return Some(args);
    }

    let mut args = state_report(&pane, "klark:kimi", "kimi", action);
    if let Some(session_id) = session_id {
        push_pairs(&mut args, &[("--agent-session-id", session_id)]);
    }
    push_pairs(&mut args, &[("--seq", seq)]);
    Some(args)
}

fn letta(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(true)?;
    let payload = Payload::parse(text);
    let fields = payload.fields()?;
    let mut session_id = text_field(fields, "conversation_id")?;
    if ieq(&session_id, "default") {
        session_id = format!("default:{}", text_field(fields, "agent_id")?);
    }
    // `$payload.is_new_session -eq $true`
    let is_new = match fields.get("is_new_session") {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => ieq(value, "true"),
        Some(Value::Number(value)) => value.as_f64() == Some(1.0),
        _ => false,
    };

    let mut args = session_report(&pane, "klark:letta", "letta");
    push_pairs(
        &mut args,
        &[
            ("--agent-session-id", session_id),
            ("--seq", ctx.dotnet_ticks()),
            (
                "--session-start-source",
                if is_new { "new" } else { "resume" }.to_string(),
            ),
        ],
    );
    Some(args)
}

fn mastracode(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !["session", "working", "idle", "blocked"]
        .iter()
        .any(|allowed| ieq(action, allowed))
    {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    let session_id = payload
        .fields()
        .and_then(|fields| nonblank_string_field(fields, "session_id"));
    let seq = ctx.dotnet_ticks();

    if ieq(action, "session") {
        let mut args = session_report(&pane, "klark:mastracode", "mastracode");
        push_pairs(
            &mut args,
            &[
                ("--seq", seq),
                ("--session-start-source", "startup".to_string()),
                ("--agent-session-id", session_id?),
            ],
        );
        return Some(args);
    }

    let mut args = state_report(&pane, "klark:mastracode", "mastracode", action);
    push_pairs(&mut args, &[("--seq", seq)]);
    if let Some(session_id) = session_id {
        push_pairs(&mut args, &[("--agent-session-id", session_id)]);
    }
    Some(args)
}

fn qwen(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(true)?;
    let payload = Payload::parse(text);
    let fields = payload.fields()?;
    let session_id = text_field(fields, "session_id")?;

    let mut args = session_report(&pane, "klark:qwen", "qwen");
    push_pairs(
        &mut args,
        &[
            ("--agent-session-id", session_id),
            ("--seq", ctx.dotnet_ticks()),
        ],
    );
    if let Some(source) = ps_text(fields.get("source")).filter(|source| {
        ["startup", "resume", "clear", "compact", "branch"]
            .iter()
            .any(|allowed| ieq(source, allowed))
    }) {
        push_pairs(&mut args, &[("--session-start-source", source)]);
    }
    Some(args)
}

/// Session-only: Antigravity CLI reports its conversation; lifecycle comes
/// from screen detection.
fn antigravity_cli(action: &str, text: &str, ctx: &HookContext<'_>) -> Option<Vec<String>> {
    if !ieq(action, "session") {
        return None;
    }
    let pane = ctx.klark_pane(false)?;
    let payload = Payload::parse(text);
    let fields = payload.fields()?;
    let conversation_id = nonblank_string_field(fields, "conversationId")?;

    let mut args = session_report(&pane, "klark:antigravity_cli", "agy");
    push_pairs(
        &mut args,
        &[
            ("--seq", ctx.unix_millis()),
            ("--agent-session-id", conversation_id),
        ],
    );
    if let Some(path) = nonblank_string_field(fields, "transcriptPath") {
        push_pairs(&mut args, &[("--agent-session-path", path)]);
    }
    Some(args)
}

fn session_report(pane: &str, source: &str, agent: &str) -> Vec<String> {
    [
        "pane",
        "report-agent-session",
        pane,
        "--source",
        source,
        "--agent",
        agent,
    ]
    .map(str::to_string)
    .to_vec()
}

fn state_report(pane: &str, source: &str, agent: &str, state: &str) -> Vec<String> {
    [
        "pane",
        "report-agent",
        pane,
        "--source",
        source,
        "--agent",
        agent,
        "--state",
        state,
    ]
    .map(str::to_string)
    .to_vec()
}

fn push_pairs(args: &mut Vec<String>, pairs: &[(&str, String)]) {
    for (flag, value) in pairs {
        args.push((*flag).to_string());
        args.push(value.clone());
    }
}

/// `[string]::IsNullOrWhiteSpace`
fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// PowerShell's case-insensitive `-eq`.
fn ieq(left: &str, right: &str) -> bool {
    left == right || left.to_lowercase() == right.to_lowercase()
}

/// A field that is a JSON string (`-is [string]`).
fn string_field<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(Value::as_str)
}

fn nonblank_string_field(fields: &Map<String, Value>, name: &str) -> Option<String> {
    string_field(fields, name)
        .filter(|value| !is_blank(value))
        .map(str::to_string)
}

/// A field the ps1 used without a type check: any scalar, as text, not blank.
fn text_field(fields: &Map<String, Value>, name: &str) -> Option<String> {
    ps_text(fields.get(name)).filter(|value| !is_blank(value))
}

/// How PowerShell turns a scalar JSON value into text. Arrays, objects and
/// `null` have no text here.
fn ps_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(true) => Some("True".to_string()),
        Value::Bool(false) => Some("False".to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// PowerShell truthiness of a JSON value (`if ($value)`).
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Number(value)) => value.as_f64() != Some(0.0),
        Some(Value::Array(values)) => !values.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

/// Runs one hook event: reads stdin, reports through `spawn`, then prints
/// the agent's stdout reply. Never fails.
pub(crate) fn run_hook(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    ctx: &HookContext<'_>,
    spawn: &mut dyn FnMut(Vec<String>, Option<Duration>),
) {
    let agent = args.first().and_then(|name| HookAgent::parse(name));
    if let Some(agent) = agent {
        let action = args.get(1).map(String::as_str).unwrap_or("");
        let mut bytes = Vec::new();
        let _ = stdin.read_to_end(&mut bytes);
        let payload = String::from_utf8_lossy(&bytes);
        if let Some(report) = hook_report_args(agent, action, &payload, ctx) {
            spawn(report, agent.report_timeout());
        }
    }
    if agent.is_some_and(HookAgent::prints_empty_json) {
        let _ = stdout.write_all(b"{}\n");
        let _ = stdout.flush();
    }
}

/// `klark integration hook <agent> [action]`. Always exits 0 and stays off
/// stderr, even on a panic, so a hook can never break or pollute the agent.
pub(crate) fn run_hook_command(args: &[String]) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    let wants_json = args
        .first()
        .and_then(|name| HookAgent::parse(name))
        .is_some_and(HookAgent::prints_empty_json);
    let mut printed = false;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let env = |name: &str| std::env::var(name).ok();
        let cwd = || std::env::current_dir().ok();
        let ctx = HookContext {
            env: &env,
            unix_nanos: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default(),
            cwd: &cwd,
            devin_list: &run_devin_list,
        };
        let mut stdout = PrintTracker {
            inner: std::io::stdout().lock(),
            printed: &mut printed,
        };
        run_hook(
            args,
            &mut std::io::stdin().lock(),
            &mut stdout,
            &ctx,
            &mut spawn_klark_report,
        );
    }));
    if result.is_err() && wants_json && !printed {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(b"{}\n");
        let _ = stdout.flush();
    }
    0
}

/// Records whether anything reached stdout, so a panic does not print `{}`
/// twice.
struct PrintTracker<'a, W: Write> {
    inner: W,
    printed: &'a mut bool,
}

impl<W: Write> Write for PrintTracker<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        *self.printed = true;
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Runs klark itself (`KLARK_BIN_PATH`, else this executable) with no stdio,
/// waiting for it, or at most `timeout`.
fn spawn_klark_report(args: Vec<String>, timeout: Option<Duration>) {
    let program = std::env::var("KLARK_BIN_PATH")
        .ok()
        .filter(|path| !is_blank(path))
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let Some(program) = program else {
        return;
    };
    let Ok(mut child) = crate::noninteractive_process::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    match timeout {
        None => {
            let _ = child.wait();
        }
        Some(timeout) => {
            wait_or_kill(&mut child, timeout);
        }
    }
}

/// Waits for `child`, killing it at `timeout`. Returns whether it finished in
/// time.
fn wait_or_kill(child: &mut std::process::Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Err(_) => return false,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// `devin list --format json`, with `devin` found on `PATH` like the
/// integration's own command lookup, so a `devin.cmd` shim works.
fn run_devin_list() -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    let program = find_command(std::env::split_paths(&paths), "devin")?;
    let mut child = crate::noninteractive_process::command(program)
        .args(["list", "--format", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        output
    });
    // Killing a `.cmd` shim ends only cmd.exe; the real process can keep the
    // pipe open, so a timed-out reader is abandoned, never joined.
    if !wait_or_kill(&mut child, DEVIN_LIST_TIMEOUT) {
        return None;
    }
    let output = reader.join().ok()?;
    Some(String::from_utf8_lossy(&output).into_owned())
}

/// The first spawnable `command` in `dirs`.
fn find_command(dirs: impl IntoIterator<Item = PathBuf>, command: &str) -> Option<PathBuf> {
    dirs.into_iter().find_map(|dir| {
        super::registry::command_path_candidates(&dir, command)
            .into_iter()
            .filter(|path| is_spawnable(path, cfg!(windows)))
            .find(|path| super::registry::executable_file_exists(path))
    })
}

/// On Windows only `.exe`, `.cmd` and `.bat` start without a shell: an
/// npm-style extensionless sh script or a `.ps1` beside `devin.cmd` cannot.
fn is_spawnable(path: &Path, windows: bool) -> bool {
    !windows
        || path.extension().is_some_and(|extension| {
            ["exe", "cmd", "bat"]
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// 2026-11-15T00:00:00.123456789Z
    const NANOS: u128 = 1_794_700_800_123_456_789;
    const MILLIS: &str = "1794700800123";
    /// .NET ticks of the same instant (days since 0001-01-01, from the calendar).
    const TICKS: &str = "639302976001234567";

    struct Fixture {
        env: HashMap<&'static str, String>,
        cwd: Option<PathBuf>,
        devin_list: Option<String>,
        devin_list_calls: RefCell<usize>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut env = HashMap::new();
            env.insert("KLARK_ENV", "1".to_string());
            env.insert("KLARK_PANE_ID", "w1:p2".to_string());
            env.insert("KLARK_SOCKET_PATH", "/tmp/klark.sock".to_string());
            Self {
                env,
                cwd: None,
                devin_list: None,
                devin_list_calls: RefCell::new(0),
            }
        }

        fn with(mut self, name: &'static str, value: &str) -> Self {
            self.env.insert(name, value.to_string());
            self
        }

        fn without(mut self, name: &'static str) -> Self {
            self.env.remove(name);
            self
        }

        fn args(&self, agent: &str, action: &str, payload: &str) -> Option<Vec<String>> {
            let env = |name: &str| self.env.get(name).cloned();
            let cwd = || self.cwd.clone();
            let devin_list = || {
                *self.devin_list_calls.borrow_mut() += 1;
                self.devin_list.clone()
            };
            let ctx = HookContext {
                env: &env,
                unix_nanos: NANOS,
                cwd: &cwd,
                devin_list: &devin_list,
            };
            hook_report_args(HookAgent::parse(agent).unwrap(), action, payload, &ctx)
        }
    }

    fn argv(parts: &[&str]) -> Option<Vec<String>> {
        Some(parts.iter().map(|part| part.to_string()).collect())
    }

    #[test]
    fn seq_units_match_the_powershell_clocks() {
        let fixture = Fixture::new();
        let env = |name: &str| fixture.env.get(name).cloned();
        let none = || None;
        let ctx = HookContext {
            env: &env,
            unix_nanos: NANOS,
            cwd: &|| None,
            devin_list: &none,
        };
        assert_eq!(ctx.unix_millis(), MILLIS);
        assert_eq!(ctx.dotnet_ticks(), TICKS);
        let epoch = HookContext {
            unix_nanos: 0,
            ..ctx
        };
        assert_eq!(epoch.dotnet_ticks(), "621355968000000000");
    }

    #[test]
    fn every_agent_needs_klark_env_and_a_pane() {
        let payload = r#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t","conversation_id":"c1","conversationId":"c1"}"#;
        for (agent, action) in [
            ("antigravity_cli", "session"),
            ("claude", "session"),
            ("codex", "session"),
            ("copilot", ""),
            ("cursor", "session"),
            ("devin", "session"),
            ("droid", "session"),
            ("grok", "session"),
            ("kimi", "session"),
            ("letta", "session"),
            ("mastracode", "session"),
            ("qodercli", "session"),
            ("qwen", "session"),
        ] {
            assert!(
                Fixture::new().args(agent, action, payload).is_some(),
                "{agent} reports with a full environment"
            );
            for fixture in [
                Fixture::new().without("KLARK_ENV"),
                Fixture::new().with("KLARK_ENV", "true"),
                Fixture::new().without("KLARK_PANE_ID"),
                Fixture::new().with("KLARK_PANE_ID", "  "),
            ] {
                assert_eq!(fixture.args(agent, action, payload), None, "{agent}");
            }
        }
    }

    #[test]
    fn copilot_letta_and_qwen_also_need_the_socket() {
        let payload = r#"{"session_id":"s1","conversation_id":"c1"}"#;
        for (agent, action) in [("copilot", ""), ("letta", "session"), ("qwen", "session")] {
            assert!(Fixture::new().args(agent, action, payload).is_some());
            let fixture = Fixture::new().with("KLARK_SOCKET_PATH", " ");
            assert_eq!(fixture.args(agent, action, payload), None, "{agent}");
        }
        let fixture = Fixture::new().without("KLARK_SOCKET_PATH");
        assert!(fixture.args("droid", "session", payload).is_some());
    }

    #[test]
    fn session_only_agents_ignore_other_actions() {
        let payload = r#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t","conversation_id":"c1","conversationId":"c1"}"#;
        for agent in [
            "antigravity_cli",
            "claude",
            "codex",
            "cursor",
            "devin",
            "droid",
            "grok",
            "letta",
            "qodercli",
            "qwen",
        ] {
            for action in ["", "working", "idle", "sessions"] {
                assert_eq!(Fixture::new().args(agent, action, payload), None, "{agent}");
            }
            // `-ne` is case-insensitive.
            assert!(
                Fixture::new().args(agent, "SESSION", payload).is_some(),
                "{agent}"
            );
        }
    }

    #[test]
    fn claude_reports_session_start_with_path_and_source() {
        let payload = r#"{"hook_event_name":"SessionStart","session_id":"abc","transcript_path":"C:\\Users\\한글\\t.jsonl","source":"resume"}"#;
        assert_eq!(
            Fixture::new().args("claude", "session", payload),
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:claude",
                "--agent",
                "claude",
                "--seq",
                MILLIS,
                "--agent-session-id",
                "abc",
                "--agent-session-path",
                "C:\\Users\\한글\\t.jsonl",
                "--session-start-source",
                "resume",
            ])
        );
        assert_eq!(
            Fixture::new().args(
                "claude",
                "session",
                r#"{"hook_event_name":"SessionStart","session_id":"abc","transcript_path":" ","source":5}"#
            ),
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:claude",
                "--agent",
                "claude",
                "--seq",
                MILLIS,
                "--agent-session-id",
                "abc",
            ])
        );
    }

    #[test]
    fn claude_guards() {
        let fixture = Fixture::new();
        for payload in [
            "",
            "not json",
            "[]",
            r#"{"session_id":"abc"}"#,
            // `-cne`: case-sensitive.
            r#"{"hook_event_name":"sessionstart","session_id":"abc"}"#,
            r#"{"hook_event_name":"Stop","session_id":"abc"}"#,
            r#"{"hook_event_name":"SessionStart","session_id":" "}"#,
            r#"{"hook_event_name":"SessionStart","session_id":"abc","agent_id":"sub"}"#,
            r#"{"hook_event_name":"SessionStart","session_id":"abc","cursor_version":"1"}"#,
        ] {
            assert_eq!(
                fixture.args("claude", "session", payload),
                None,
                "{payload}"
            );
        }
        let payload = r#"{"hook_event_name":"SessionStart","session_id":"abc"}"#;
        let cursor = Fixture::new().with("CURSOR_VERSION", "1.2");
        assert_eq!(cursor.args("claude", "session", payload), None);
        // A numeric id is stringified, as `"$sessionId"` does.
        let numeric = r#"{"hook_event_name":"SessionStart","session_id":42}"#;
        let args = fixture.args("claude", "session", numeric).unwrap();
        assert_eq!(args[args.len() - 1], "42");
        // UTF-8 BOM dropped, as StreamReader does.
        assert!(fixture
            .args("claude", "session", &format!("\u{feff}{payload}"))
            .is_some());
    }

    #[test]
    fn codex_reports_with_ticks_and_checks_its_thread() {
        let payload = r#"{"hook_event_name":"SessionStart","session_id":"t1","transcript_path":"/r.jsonl","source":"startup"}"#;
        assert_eq!(
            Fixture::new().args("codex", "session", payload),
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:codex",
                "--agent",
                "codex",
                "--seq",
                TICKS,
                "--agent-session-id",
                "t1",
                "--session-start-source",
                "startup",
            ])
        );
        // No event name: allowed, but no start source.
        let args = Fixture::new()
            .args(
                "codex",
                "session",
                r#"{"session_id":"t1","transcript_path":"/r","source":"startup"}"#,
            )
            .unwrap();
        assert!(!args.contains(&"--session-start-source".to_string()));
        // `-ne` is case-insensitive.
        assert!(Fixture::new()
            .args(
                "codex",
                "session",
                r#"{"hook_event_name":"sessionstart","session_id":"t1","transcript_path":"/r"}"#
            )
            .is_some());
        for payload in [
            r#"{"hook_event_name":"Stop","session_id":"t1","transcript_path":"/r"}"#,
            r#"{"session_id":"t1"}"#,
            r#"{"session_id":"t1","transcript_path":"  "}"#,
            r#"{"transcript_path":"/r"}"#,
            "{",
            "",
        ] {
            assert_eq!(
                Fixture::new().args("codex", "session", payload),
                None,
                "{payload}"
            );
        }
        let payload = r#"{"session_id":"t1","transcript_path":"/r"}"#;
        let other_thread = Fixture::new().with("CODEX_THREAD_ID", "t2");
        assert_eq!(other_thread.args("codex", "session", payload), None);
        let same_thread = Fixture::new().with("CODEX_THREAD_ID", "T1");
        assert!(same_thread.args("codex", "session", payload).is_some());
    }

    #[test]
    fn copilot_normalises_events_and_skips_lifecycle_payloads() {
        let expected = argv(&[
            "pane",
            "report-agent-session",
            "w1:p2",
            "--source",
            "klark:copilot",
            "--agent",
            "copilot",
            "--agent-session-id",
            "cp1",
            "--seq",
            MILLIS,
        ]);
        for payload in [
            r#"{"hook_event_name":"session_start","session_id":"cp1"}"#,
            r#"{"hookEventName":"Session-Start","sessionId":"cp1"}"#,
            r#"{"hook_event_name":"SessionStart","session_id":" ","sessionId":"cp1"}"#,
            r#"{"sessionId":"cp1"}"#,
        ] {
            assert_eq!(
                Fixture::new().args("copilot", "", payload),
                expected,
                "{payload}"
            );
        }
        // Copilot passes no action, but any is ignored.
        assert_eq!(
            Fixture::new().args("copilot", "whatever", r#"{"sessionId":"cp1"}"#),
            expected
        );
        for payload in [
            r#"{"hook_event_name":"userPromptSubmitted","session_id":"cp1"}"#,
            r#"{"prompt":"","session_id":"cp1"}"#,
            r#"{"Prompt":"hi","session_id":"cp1"}"#,
            r#"{"toolName":"bash","session_id":"cp1"}"#,
            r#"{"reason":"done","session_id":"cp1"}"#,
            r#"{"stop_reason":"end","sessionId":"cp1"}"#,
            "",
            "not json",
        ] {
            assert_eq!(
                Fixture::new().args("copilot", "", payload),
                None,
                "{payload}"
            );
        }
        // A blank tool name is not text, so the payload still reports.
        assert_eq!(
            Fixture::new().args("copilot", "", r#"{"tool_name":" ","sessionId":"cp1"}"#),
            expected
        );
    }

    #[test]
    fn cursor_skips_to_the_json_and_takes_the_first_id() {
        let expected = argv(&[
            "pane",
            "report-agent-session",
            "w1:p2",
            "--source",
            "klark:cursor",
            "--agent",
            "cursor",
            "--seq",
            TICKS,
            "--agent-session-id",
            "cv1",
        ]);
        for payload in [
            r#"{"hook_event_name":"sessionStart","session_id":"cv1"}"#,
            r#"noise before {"hook_event_name":"sessionStart","conversation_id":"cv1"}"#,
            r#"{"hook_event_name":"SessionStart","conversationId":"cv1"}"#,
            r#"{"hookEventName":"sessionStart","sessionId":"cv1","conversation_id":"x"}"#,
            r#"{"session_id":7,"sessionId":"cv1"}"#,
            r#"{"hook_event_name":5,"hookEventName":"sessionStart","session_id":"cv1"}"#,
        ] {
            assert_eq!(
                Fixture::new().args("cursor", "session", payload),
                expected,
                "{payload}"
            );
        }
        for payload in [
            r#"{"hook_event_name":"stop","session_id":"cv1"}"#,
            r#"{"session_id":" "}"#,
            "null",
            "",
            "no json here",
        ] {
            assert_eq!(
                Fixture::new().args("cursor", "session", payload),
                None,
                "{payload}"
            );
        }
    }

    #[test]
    fn devin_uses_the_payload_id_then_devin_list() {
        let expected = |id: &str| {
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:devin",
                "--agent",
                "devin",
                "--seq",
                TICKS,
                "--agent-session-id",
                id,
            ])
        };
        let fixture = Fixture::new();
        assert_eq!(
            fixture.args("devin", "session", r#"{"session_id":"d1"}"#),
            expected("d1")
        );
        assert_eq!(
            fixture.args("devin", "session", r#"{"sessionId":"d2"}"#),
            expected("d2")
        );
        assert_eq!(*fixture.devin_list_calls.borrow(), 0);

        let project = std::env::temp_dir().join("klark-devin-project");
        let list = serde_json::json!([
            {"id": 3, "working_directory": project},
            {"id": "other", "working_directory": "/somewhere/else"},
            {"id": "d3", "working_directory": project.to_string_lossy().to_uppercase()},
        ])
        .to_string();
        let mut from_list = Fixture::new().with("DEVIN_PROJECT_DIR", &project.to_string_lossy());
        from_list.devin_list = Some(list.clone());
        // Matched ignoring case; ids that are not strings are skipped.
        assert_eq!(
            from_list.args("devin", "session", r#"{"hook_event_name":"Stop"}"#),
            expected("d3")
        );
        // A blank string session_id hides sessionId (the ps1's `elseif`)
        // and falls back to the list.
        assert_eq!(
            from_list.args(
                "devin",
                "session",
                r#"{"session_id":"","sessionId":"ignored"}"#
            ),
            expected("d3")
        );
        // Empty and unparsable payloads also fall back.
        assert_eq!(from_list.args("devin", "session", ""), expected("d3"));
        assert_eq!(from_list.args("devin", "session", "{"), expected("d3"));

        // No fallback on a prompt or a fresh startup.
        for payload in [
            r#"{"hook_event_name":"UserPromptSubmit"}"#,
            r#"{"hook_event_name":"sessionstart","source":"Startup"}"#,
        ] {
            assert_eq!(
                from_list.args("devin", "session", payload),
                None,
                "{payload}"
            );
        }
        // Four lookups: Stop, blank id, empty and unparsable payloads.
        assert_eq!(*from_list.devin_list_calls.borrow(), 4);

        // The current directory when DEVIN_PROJECT_DIR is unset.
        let mut from_cwd = Fixture::new();
        from_cwd.cwd = Some(project.clone());
        from_cwd.devin_list = Some(list);
        assert_eq!(from_cwd.args("devin", "session", "{}"), expected("d3"));

        let mut no_match = Fixture::new().with("DEVIN_PROJECT_DIR", "/no/such/project");
        no_match.devin_list = Some(r#"[{"id":"x","working_directory":"/elsewhere"}]"#.into());
        assert_eq!(no_match.args("devin", "session", "{}"), None);
        let mut single = Fixture::new().with("DEVIN_PROJECT_DIR", "/p");
        single.devin_list = Some(r#"{"id":"one","working_directory":"/p"}"#.into());
        assert_eq!(single.args("devin", "session", "{}"), expected("one"));
        let mut failed = Fixture::new().with("DEVIN_PROJECT_DIR", "/p");
        failed.devin_list = None;
        assert_eq!(failed.args("devin", "session", "{}"), None);
    }

    #[test]
    fn droid_and_qodercli_report_the_session_id() {
        for (agent, source) in [("droid", "klark:droid"), ("qodercli", "klark:qodercli")] {
            assert_eq!(
                Fixture::new().args(agent, "session", r#"{"session_id":"s9"}"#),
                argv(&[
                    "pane",
                    "report-agent-session",
                    "w1:p2",
                    "--source",
                    source,
                    "--agent",
                    agent,
                    "--agent-session-id",
                    "s9",
                    "--seq",
                    MILLIS,
                ])
            );
            for payload in [
                "",
                "{",
                "null",
                r#"{"session_id":" "}"#,
                r#"{"sessionId":"x"}"#,
            ] {
                assert_eq!(
                    Fixture::new().args(agent, "session", payload),
                    None,
                    "{agent}"
                );
            }
            // No `-is [string]` check in the ps1: a number passes.
            assert!(Fixture::new()
                .args(agent, "session", r#"{"session_id":12}"#)
                .is_some());
        }
    }

    #[test]
    fn grok_prefers_its_env_session_and_filters_events() {
        let expected = |id: &str| {
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:grok",
                "--agent",
                "grok",
                "--seq",
                TICKS,
                "--agent-session-id",
                id,
                "--session-start-source",
                "new",
            ])
        };
        let payload = r#"{"hook_event_name":"session_start","session_id":"g1","source":"new"}"#;
        assert_eq!(
            Fixture::new().args("grok", "session", payload),
            expected("g1")
        );
        let env = Fixture::new().with("GROK_SESSION_ID", "genv");
        assert_eq!(env.args("grok", "session", payload), expected("genv"));
        assert_eq!(
            Fixture::new().args(
                "grok",
                "session",
                r#"{"hookEventName":"SESSIONSTART","sessionId":"g1","source":"new"}"#
            ),
            expected("g1")
        );
        let no_source = Fixture::new()
            .args("grok", "session", r#"{"session_id":"g1","source":" "}"#)
            .unwrap();
        assert!(!no_source.contains(&"--session-start-source".to_string()));
        assert_eq!(
            env.args("grok", "session", r#"{"hook_event_name":"Stop"}"#),
            None
        );
        assert_eq!(Fixture::new().args("grok", "session", "{}"), None);
        assert_eq!(env.args("grok", "session", "").unwrap()[10], "genv");
    }

    #[test]
    fn kimi_reports_session_and_states() {
        assert_eq!(
            Fixture::new().args("kimi", "session", r#"{"session_id":"k1"}"#),
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:kimi",
                "--agent",
                "kimi",
                "--agent-session-id",
                "k1",
                "--session-start-source",
                "startup",
                "--seq",
                MILLIS,
            ])
        );
        assert_eq!(Fixture::new().args("kimi", "session", "{}"), None);
        for state in ["working", "blocked", "idle"] {
            assert_eq!(
                Fixture::new().args("kimi", state, r#"{"session_id":"k1"}"#),
                argv(&[
                    "pane",
                    "report-agent",
                    "w1:p2",
                    "--source",
                    "klark:kimi",
                    "--agent",
                    "kimi",
                    "--state",
                    state,
                    "--agent-session-id",
                    "k1",
                    "--seq",
                    MILLIS,
                ])
            );
            assert_eq!(
                Fixture::new().args("kimi", state, "garbage"),
                argv(&[
                    "pane",
                    "report-agent",
                    "w1:p2",
                    "--source",
                    "klark:kimi",
                    "--agent",
                    "kimi",
                    "--state",
                    state,
                    "--seq",
                    MILLIS,
                ])
            );
        }
        assert_eq!(Fixture::new().args("kimi", "release", "{}"), None);
    }

    #[test]
    fn mastracode_reports_session_and_states() {
        assert_eq!(
            Fixture::new().args("mastracode", "session", r#"{"session_id":"m1"}"#),
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:mastracode",
                "--agent",
                "mastracode",
                "--seq",
                TICKS,
                "--session-start-source",
                "startup",
                "--agent-session-id",
                "m1",
            ])
        );
        // `-is [string]`: a numeric id is no id.
        assert_eq!(
            Fixture::new().args("mastracode", "session", r#"{"session_id":1}"#),
            None
        );
        assert_eq!(
            Fixture::new().args("mastracode", "blocked", r#"{"session_id":"m1"}"#),
            argv(&[
                "pane",
                "report-agent",
                "w1:p2",
                "--source",
                "klark:mastracode",
                "--agent",
                "mastracode",
                "--state",
                "blocked",
                "--seq",
                TICKS,
                "--agent-session-id",
                "m1",
            ])
        );
        assert_eq!(
            Fixture::new().args("mastracode", "idle", "").unwrap().len(),
            11
        );
        assert_eq!(Fixture::new().args("mastracode", "release", "{}"), None);
    }

    #[test]
    fn letta_reports_conversation_with_new_or_resume() {
        let expected = |id: &str, source: &str| {
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:letta",
                "--agent",
                "letta",
                "--agent-session-id",
                id,
                "--seq",
                TICKS,
                "--session-start-source",
                source,
            ])
        };
        let fixture = Fixture::new();
        assert_eq!(
            fixture.args(
                "letta",
                "session",
                r#"{"conversation_id":"conv-1","is_new_session":true}"#
            ),
            expected("conv-1", "new")
        );
        assert_eq!(
            fixture.args("letta", "session", r#"{"conversation_id":"conv-1"}"#),
            expected("conv-1", "resume")
        );
        assert_eq!(
            fixture.args(
                "letta",
                "session",
                r#"{"conversation_id":"default","agent_id":"agent-7","is_new_session":false}"#
            ),
            expected("default:agent-7", "resume")
        );
        for payload in [
            r#"{"conversation_id":"default"}"#,
            r#"{"conversation_id":" "}"#,
            "",
            "{",
        ] {
            assert_eq!(fixture.args("letta", "session", payload), None, "{payload}");
        }
        assert_eq!(
            HookAgent::Letta.report_timeout(),
            Some(Duration::from_secs(1))
        );
        assert_eq!(HookAgent::Claude.report_timeout(), None);
    }

    #[test]
    fn qwen_passes_only_known_start_sources() {
        let base = [
            "pane",
            "report-agent-session",
            "w1:p2",
            "--source",
            "klark:qwen",
            "--agent",
            "qwen",
            "--agent-session-id",
            "q1",
            "--seq",
            TICKS,
        ];
        for source in ["startup", "resume", "clear", "compact", "branch", "Resume"] {
            let mut expected = base.to_vec();
            expected.extend(["--session-start-source", source]);
            assert_eq!(
                Fixture::new().args(
                    "qwen",
                    "session",
                    &format!(r#"{{"session_id":"q1","source":"{source}"}}"#)
                ),
                argv(&expected)
            );
        }
        assert_eq!(
            Fixture::new().args("qwen", "session", r#"{"session_id":"q1","source":"other"}"#),
            argv(&base)
        );
        assert_eq!(Fixture::new().args("qwen", "session", "{}"), None);
    }

    #[test]
    fn antigravity_reports_conversation_and_transcript() {
        assert_eq!(
            Fixture::new().args(
                "antigravity_cli",
                "session",
                r#"{"conversationId":"ag1","transcriptPath":"/t/ag1.jsonl"}"#
            ),
            argv(&[
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "klark:antigravity_cli",
                "--agent",
                "agy",
                "--seq",
                MILLIS,
                "--agent-session-id",
                "ag1",
                "--agent-session-path",
                "/t/ag1.jsonl",
            ])
        );
        for payload in [
            r#"{"conversationId":7}"#,
            r#"{"conversation_id":"x"}"#,
            "",
            "{",
        ] {
            assert_eq!(
                Fixture::new().args("antigravity_cli", "session", payload),
                None,
                "{payload}"
            );
        }
    }

    /// A captured report: the klark argv and its time cap.
    type Spawned = (Vec<String>, Option<Duration>);

    /// Runs the hook with a capturing spawner; returns stdout and spawns.
    fn run(fixture: &Fixture, args: &[&str], stdin: &[u8]) -> (String, Vec<Spawned>) {
        let env = |name: &str| fixture.env.get(name).cloned();
        let ctx = HookContext {
            env: &env,
            unix_nanos: NANOS,
            cwd: &|| None,
            devin_list: &|| None,
        };
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let mut stdout = Vec::new();
        let mut spawned = Vec::new();
        run_hook(
            &args,
            &mut &stdin[..],
            &mut stdout,
            &ctx,
            &mut |argv, timeout| spawned.push((argv, timeout)),
        );
        (String::from_utf8(stdout).unwrap(), spawned)
    }

    #[test]
    fn devin_lookup_skips_files_windows_cannot_spawn() {
        for (name, windows_ok) in [
            ("devin", false),
            ("devin.ps1", false),
            ("devin.PS1", false),
            ("devin.cmd", true),
            ("devin.CMD", true),
            ("devin.bat", true),
            ("devin.exe", true),
        ] {
            assert_eq!(is_spawnable(Path::new(name), true), windows_ok, "{name}");
            assert!(is_spawnable(Path::new(name), false), "{name} on unix");
        }

        let base = std::env::temp_dir().join(format!(
            "klark-hook-run-find-{}-{}",
            std::process::id(),
            NANOS
        ));
        let empty = base.join("empty");
        let bin = base.join("bin");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let devin = bin.join("devin");
        std::fs::write(&devin, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&devin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        if !cfg!(windows) {
            assert_eq!(
                find_command([empty.clone(), bin.clone()], "devin"),
                Some(devin)
            );
        }
        assert_eq!(find_command([empty], "devin"), None);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn cursor_and_antigravity_print_one_empty_object_on_every_path() {
        for (agent, payload) in [
            ("cursor", r#"{"session_id":"cv1"}"#),
            ("antigravity_cli", r#"{"conversationId":"ag1"}"#),
        ] {
            for (fixture, action, stdin, reports) in [
                (Fixture::new(), "session", payload, true),
                (Fixture::new(), "working", payload, false),
                (Fixture::new(), "session", "not json", false),
                (Fixture::new(), "session", "", false),
                (
                    Fixture::new().without("KLARK_ENV"),
                    "session",
                    payload,
                    false,
                ),
            ] {
                let (stdout, spawned) = run(&fixture, &[agent, action], stdin.as_bytes());
                assert_eq!(stdout, "{}\n", "{agent} {action} {stdin:?}");
                assert_eq!(stdout.matches("{}").count(), 1);
                assert_eq!(spawned.len(), usize::from(reports), "{agent}");
            }
            let (stdout, _) = run(&Fixture::new(), &[agent], b"");
            assert_eq!(stdout, "{}\n", "{agent} without an action");
        }
    }

    #[test]
    fn other_agents_print_nothing() {
        let payload =
            br#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t"}"#;
        for agent in [
            "claude",
            "codex",
            "copilot",
            "devin",
            "droid",
            "grok",
            "kimi",
            "letta",
            "mastracode",
            "qodercli",
            "qwen",
        ] {
            let (stdout, _) = run(&Fixture::new(), &[agent, "session"], payload);
            assert_eq!(stdout, "", "{agent}");
        }
        let (stdout, spawned) = run(&Fixture::new(), &["unknown-agent", "session"], payload);
        assert_eq!(stdout, "");
        assert!(spawned.is_empty());
        let (stdout, spawned) = run(&Fixture::new(), &[], payload);
        assert_eq!(stdout, "");
        assert!(spawned.is_empty());
    }

    #[test]
    fn run_hook_decodes_stdin_as_utf8_and_passes_letta_its_timeout() {
        // A Korean profile path must survive (the old PowerShell hooks broke
        // on CP949 consoles; see #5253). Invalid bytes are replaced, not fatal.
        let transcript = "C:\\Users\\한글\\transcript.jsonl";
        let payload = format!(
            "\u{feff}{{\"hook_event_name\":\"SessionStart\",\"session_id\":\"claude-session\",\"transcript_path\":{}}}",
            serde_json::to_string(transcript).unwrap()
        );
        let (_, spawned) = run(&Fixture::new(), &["claude", "session"], payload.as_bytes());
        assert_eq!(spawned.len(), 1);
        assert!(spawned[0].0.contains(&transcript.to_string()));
        assert_eq!(spawned[0].1, None);

        let mut bytes = br#"{"session_id":"s1","x":""#.to_vec();
        bytes.extend([0xff, 0xfe]);
        bytes.extend(br#""}"#);
        let (_, spawned) = run(&Fixture::new(), &["droid", "session"], &bytes);
        assert_eq!(spawned.len(), 1);

        let (_, spawned) = run(
            &Fixture::new(),
            &["letta", "session"],
            br#"{"conversation_id":"c1"}"#,
        );
        assert_eq!(spawned[0].1, Some(Duration::from_secs(1)));
    }
}
