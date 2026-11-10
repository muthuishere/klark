using System.Text.Json.Nodes;
using KlarkHub.Services;

namespace KlarkHub.Tests;

public class TranscriptCleanTests
{
    private static string Fixture(string name) => File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "Fixtures", name));

    // Every piece of TUI chrome named in the Claude Code fixture, none of which may survive.
    public static TheoryData<string> ClaudeChrome => new()
    {
        "Welcome to Claude Code", "/help for help", "cwd: /Users/dev/app", "Tip:", "Thinking…", "Cogitating…",
        "Sautéed for", "? for shortcuts", "Context left until auto-compact", "accept edits on", "plan mode on",
        "bypass permissions on", "shift+tab", "Total cost", "Total duration", "23.4k tokens", "Claude Code v2",
        "───", "━━━", "═══", "- - - -", "╭", "╰", "│", "ctrl+r to expand", "esc to interrupt",
    };

    [Theory]
    [MemberData(nameof(ClaudeChrome))]
    public void ClaudeCodeScreen_DropsChrome(string chrome)
    {
        var cleaned = TranscriptClean.Clean(Fixture("claude-code.screen.txt"));
        Assert.DoesNotContain(chrome, cleaned);
    }

    [Fact]
    public void ClaudeCodeScreen_KeepsContent()
    {
        var cleaned = TranscriptClean.Clean(Fixture("claude-code.screen.txt"));
        foreach (var keep in new[]
                 {
                     "> fix the login bug", "⏺ I'll look at the auth module first.", "⏺ Read(src/auth.rs)",
                     "⎿  Read 120 lines", "⏺ Bash(cargo test auth)", "test result: ok. 4 passed", "… +12 lines",
                     "| auth.rs | token check |", "|------|--------|", "  ---", "```rust", "// ----------------", "Done.",
                 })
        {
            Assert.Contains(keep, cleaned);
        }
        // The empty prompt line is gone, and nothing trails.
        Assert.DoesNotMatch(@"(?m)^\s*>\s*$", cleaned);
        Assert.Equal(cleaned.TrimEnd(), cleaned);
    }

    public static TheoryData<string> DevinChrome => new()
    {
        "Devin CLI v1", "Welcome to Devin", "Working (5s", "Thinking…", "⏎ send", "87% context left",
        "Context: 41% used", "Ctrl+C again", "ctrl+o to expand", "────",
    };

    [Theory]
    [MemberData(nameof(DevinChrome))]
    public void DevinScreen_DropsChrome(string chrome)
    {
        var cleaned = TranscriptClean.Clean(Fixture("devin.screen.txt"));
        Assert.DoesNotContain(chrome, cleaned);
    }

    [Fact]
    public void DevinScreen_SplitsIntoTurns()
    {
        var turns = TranscriptClean.Turns(TranscriptClean.Clean(Fixture("devin.screen.txt")));
        Assert.Collection(turns,
            t => Assert.Equal(("user", "text", "add a health endpoint"), (t.Role, t.Kind, t.Text)),
            t => Assert.Equal(("assistant", "text", "I'll add /healthz to the router."), (t.Role, t.Kind, t.Text)),
            t => Assert.Equal(("tool_use", "Edit", "src/router.go"), (t.Kind, t.Tool, t.Summary)),
            t => Assert.Equal(("tool_result", "Added 6 lines"), (t.Kind, t.Text)),
            t => Assert.Equal(("tool_use", "Bash", "go test ./..."), (t.Kind, t.Tool, t.Summary)),
            t => Assert.Equal(("tool_result", "ok   app/router  0.21s"), (t.Kind, t.Text)),
            t => Assert.Equal(("assistant", "text", "The endpoint is in place and tests pass."), (t.Role, t.Kind, t.Text)));
    }

    [Fact]
    public void CodexScreen_DropsChromeKeepsTurns()
    {
        var cleaned = TranscriptClean.Clean(Fixture("codex.screen.txt"));
        foreach (var chrome in new[] { "OpenAI Codex", "/model to change", "directory:", "To get started", "▌", "⏎ send", "context left", "Token usage" })
            Assert.DoesNotContain(chrome, cleaned);
        var turns = TranscriptClean.Turns(cleaned);
        Assert.Equal(2, turns.Count);
        Assert.Equal(("user", "explain main.go"), (turns[0].Role, turns[0].Text));
        Assert.Equal("main.go wires the HTTP server and the config loader.", turns[1].Text);
    }

    [Fact]
    public void ClaudeCodeScreen_SplitsIntoTurnsWithToolRows()
    {
        var turns = TranscriptClean.Turns(TranscriptClean.Clean(Fixture("claude-code.screen.txt")));
        Assert.Equal("user", turns[0].Role);
        Assert.Equal("fix the login bug", turns[0].Text);
        Assert.Equal(("text", "I'll look at the auth module first."), (turns[1].Kind, turns[1].Text));
        Assert.Equal(("tool_use", "Read", "src/auth.rs"), (turns[2].Kind, turns[2].Tool, turns[2].Summary));
        Assert.Equal(("tool_result", "Read 120 lines"), (turns[3].Kind, turns[3].Text));
        Assert.Equal(("tool_use", "Bash", "cargo test auth"), (turns[4].Kind, turns[4].Tool, turns[4].Summary));
        Assert.Equal("tool_result", turns[5].Kind);
        Assert.Equal("running 4 tests\ntest result: ok. 4 passed\n… +12 lines", turns[5].Text);
        var answer = turns[6];
        Assert.Equal("text", answer.Kind);
        Assert.StartsWith("All 4 auth tests pass. Summary:", answer.Text);
        // Dedented, so markdown sees the table, the rule and the fence.
        Assert.Contains("\n| file | change |\n|------|--------|\n", answer.Text);
        Assert.Contains("\n---\n", answer.Text);
        Assert.Contains("\n```rust\nfn login() {\n    // ----------------\n}\n```", answer.Text);
        Assert.EndsWith("Done.", answer.Text);
        Assert.Equal(7, turns.Count);
    }

    [Fact]
    public void ScreenItems_RenderToolRowsAndStableIds()
    {
        var cleaned = TranscriptClean.Clean(Fixture("claude-code.screen.txt"));
        var a = TranscriptFeeds.ScreenItems(cleaned);
        var b = TranscriptFeeds.ScreenItems(cleaned + "\n\n> another question");
        Assert.Equal(a.Select(i => i.Id), b.Take(a.Count).Select(i => i.Id));
        Assert.Equal("user", b[^1].Role);
        Assert.Equal("Bash", a.First(i => i.Kind == "tool_use" && i.Tool == "Bash").Tool);
        var rows = KlarkHub.Components.Pages.ChatRowModel.Build(a);
        // user, text, one folded tool row (Read+Bash and results), the answer.
        Assert.Equal(new[] { "user", "text", "tools", "text" }, rows.Select(r => r.Kind));
    }

    // ---- negatives: real content that looks a little like chrome ----

    [Theory]
    [InlineData("---")]
    [InlineData("> a genuine quoted line")]
    [InlineData("The build takes about 30s on this machine.")]
    [InlineData("It used 500 tokens to do that.")]
    [InlineData("We are done with the migration.")]
    [InlineData("Press Esc to close the dialog in the app we built.")]
    [InlineData("| a | b |")]
    [InlineData("┌───┬───┐")]
    [InlineData("│ a │ b │")]
    [InlineData("model accuracy is 41%")]
    [InlineData("1. Yes")]
    public void KeepsContent(string line) => Assert.Equal(line, TranscriptClean.Clean(line));

    [Fact]
    public void KeepsEverythingInsideAFence()
    {
        var text = "```\n────────────────\n❯\n? for shortcuts\n```";
        Assert.Equal(text, TranscriptClean.Clean(text));
    }

    [Fact]
    public void KeepsAPromptThatWasSentInsideTheInputBox()
    {
        var text = "╭──────────────╮\n│ > deploy it  │\n╰──────────────╯";
        Assert.Equal("> deploy it", TranscriptClean.Clean(text));
    }

    [Fact]
    public void KeepsAPermissionDialogBoxForQuickReplies()
    {
        var text = "╭──────────────────────────────╮\n│ Do you want to proceed?      │\n│ ❯ 1. Yes                     │\n│   2. No                      │\n╰──────────────────────────────╯";
        var cleaned = TranscriptClean.Clean(text);
        var b = TranscriptClean.Blocked(cleaned);
        Assert.Equal("Do you want to proceed?", b.Question);
        Assert.Equal(new[] { "1", "2" }, b.Options.Select(o => o.Send));
    }

    [Fact]
    public void StripsAnsi() => Assert.Equal("hi", TranscriptClean.Clean("\u001b[1;32mhi\u001b[0m\n\u001b[2m? for shortcuts\u001b[0m"));

    // ---- structured (jsonl/db) path ----

    [Theory]
    [InlineData("<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>", "/clear")]
    [InlineData("<command-name>/model</command-name><command-args>opus</command-args>", "/model opus")]
    [InlineData("<local-command-stdout>Set model to opus</local-command-stdout>", "")]
    [InlineData("<system-reminder>\nnoise\n</system-reminder>", "")]
    [InlineData("fix it\n<system-reminder>be careful</system-reminder>", "fix it")]
    [InlineData("Caveat: The messages below were generated by the user while running local commands. DO NOT respond.", "")]
    [InlineData("<bash-input>ls</bash-input>", "! ls")]
    [InlineData("plain text", "plain text")]
    [InlineData("", "")]
    public void CleanStructured(string input, string want) => Assert.Equal(want, TranscriptClean.CleanStructured(input));

    [Theory]
    [InlineData("""{"role":"system","kind":"text","text":"hook ran"}""")]
    [InlineData("""{"role":"user","kind":"meta","text":"x"}""")]
    [InlineData("""{"role":"user","kind":"text","text":"x","meta":true}""")]
    [InlineData("""{"role":"user","kind":"text","text":"<system-reminder>r</system-reminder>"}""")]
    [InlineData("""{"role":"assistant","kind":"text","text":"   "}""")]
    [InlineData("""{"role":"assistant","kind":"text"}""")]
    [InlineData("""{"role":"assistant","kind":"thinking","text":""}""")]
    public void StructuredItems_NeverShowSystemMetaOrEmpty(string json) =>
        Assert.Null(TranscriptFeeds.ToItem((JsonObject)JsonNode.Parse(json)!, 0));

    [Fact]
    public void CodexModelChangeNote_IsChrome() =>
        Assert.Equal("› hi", TranscriptClean.Clean("Model changed to gpt-6-luna low\n\n› hi"));

    [Fact]
    public void StructuredItems_KeepRealTurnsAndTools()
    {
        Assert.Equal("/clear", TranscriptFeeds.ToItem((JsonObject)JsonNode.Parse("""{"role":"user","kind":"text","text":"<command-name>/clear</command-name>"}""")!, 0)!.Text);
        Assert.NotNull(TranscriptFeeds.ToItem((JsonObject)JsonNode.Parse("""{"role":"assistant","kind":"tool_use","tool":"Bash","input":{"command":"ls"}}""")!, 0));
        Assert.NotNull(TranscriptFeeds.ToItem((JsonObject)JsonNode.Parse("""{"role":"user","kind":"tool_result","text":""}""")!, 0));
        // The machine sends tool results as role "tool" (src/transcript_cmd.rs).
        Assert.NotNull(TranscriptFeeds.ToItem((JsonObject)JsonNode.Parse("""{"role":"tool","kind":"tool_result","text":"ok"}""")!, 0));
        Assert.Null(TranscriptFeeds.ToItem((JsonObject)JsonNode.Parse("""{"role":"tool","kind":"text","text":"x"}""")!, 0));
    }
}
