using KlarkHub.Components.Pages;
using KlarkHub.Services;

namespace KlarkHub.Tests;

/// <summary>The three screen transformers (ADR 0011, F), one fixture per pane type.</summary>
public class ScreenTransformerTests
{
    private static string Fixture(string name) => File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "Fixtures", name));

    [Theory]
    [InlineData("claude", "claude-code.screen.txt", ScreenKind.Claude)]
    [InlineData("", "claude-code.screen.txt", ScreenKind.Claude)]
    [InlineData("devin", "devin.screen.txt", ScreenKind.Devin)]
    [InlineData("", "devin.screen.txt", ScreenKind.Devin)]
    [InlineData("codex", "codex.screen.txt", ScreenKind.Codex)]
    [InlineData("", "codex-idle.screen.txt", ScreenKind.Codex)]
    [InlineData("agy", "agy.screen.txt", ScreenKind.Agy)]
    [InlineData("Antigravity", "agy.screen.txt", ScreenKind.Agy)]
    [InlineData("", "agy.screen.txt", ScreenKind.Agy)]
    [InlineData("", "generic-shell.screen.txt", ScreenKind.Generic)]
    [InlineData("aider", "generic-agent.screen.txt", ScreenKind.Generic)]
    public void PicksTheTransformerByAgentThenByScreen(string agent, string fixture, ScreenKind want) =>
        Assert.Equal(want, ScreenTransformers.Pick(agent, Fixture(fixture)));

    [Fact]
    public void Claude_TurnsToolsAndResults()
    {
        var turns = ScreenTransformers.Claude(Fixture("claude-code.screen.txt"));
        Assert.Equal(("user", "fix the login bug"), (turns[0].Role, turns[0].Text));
        Assert.Equal(("tool_use", "Read", "src/auth.rs"), (turns[2].Kind, turns[2].Tool, turns[2].Summary));
        Assert.Equal(("tool_use", "Bash", "cargo test auth"), (turns[4].Kind, turns[4].Tool, turns[4].Summary));
        Assert.StartsWith("All 4 auth tests pass", turns[^1].Text);
        Assert.DoesNotContain(turns, t => t.Text.Contains("for shortcuts") || t.Text.Contains("Welcome to Claude"));
    }

    [Fact]
    public void Claude_BoxDrawnTableBecomesARealTable()
    {
        var turns = ScreenTransformers.Claude(Fixture("claude-table.screen.txt"));
        Assert.Equal(("user", "compare the transports"), (turns[0].Role, turns[0].Text));
        var answer = turns[1].Text;
        Assert.Contains("| Transport | Direction | Status |\n|---|---|---|\n| NDJSON | hub → machine events | shipped |", answer);
        // A cell wrapped over two lines between separators is one cell.
        Assert.Contains("| POST segments | machine → hub segments, gzip | shipped |", answer);
        Assert.DoesNotContain("┌", answer);
        var items = TranscriptFeeds.ScreenItems(turns, "box", "p1");
        var html = string.Concat(items[1].Md!.OfType<MdHtml>().Select(h => h.Html));
        Assert.Contains("<table>", html);
        Assert.Contains("<th>Transport</th>", html);
        // The path:line in the answer is a viewer link.
        Assert.Contains("href=\"/m/box/f?pane=p1&amp;path=crates%2Fklark-link%2Fsrc%2Ftransport.rs#L42\"", html);
        // A screen result that reads as an error marks the call failed.
        Assert.False(items.Single(i => i.Kind == "tool_result").Ok);
    }

    [Fact]
    public void Devin_TurnsWithItsOwnMarkers()
    {
        var turns = ScreenTransformers.Devin(Fixture("devin.screen.txt"));
        Assert.Equal(("user", "add a health endpoint"), (turns[0].Role, turns[0].Text));
        Assert.Equal("I'll add /healthz to the router.", turns[1].Text);
        Assert.Equal(("tool_use", "Edit", "src/router.go"), (turns[2].Kind, turns[2].Tool, turns[2].Summary));
        Assert.Equal(("tool_result", "Added 6 lines"), (turns[3].Kind, turns[3].Text));
        Assert.Equal(("tool_use", "Bash", "go test ./..."), (turns[4].Kind, turns[4].Tool, turns[4].Summary));
        Assert.Equal("The endpoint is in place and tests pass.", turns[^1].Text);
        Assert.DoesNotContain(turns, t => t.Text.Contains("context left") || t.Text.Contains("Ctrl+C") || t.Text.Contains("Working"));
    }

    [Fact]
    public void Codex_IdleScreenDropsPlaceholderFooterAndPreamble()
    {
        var turns = ScreenTransformers.Codex(Fixture("codex-idle.screen.txt"));
        Assert.Equal(new[] { ("user", "hi"), ("assistant", "Hi! What can I help you with?") }, turns.Select(t => (t.Role, t.Text)));
        foreach (var gone in new[] { "Ask Codex to do anything", "for agents", "Model changed", "loose ends", "permissions", "/private/tmp", "Worked for" })
            Assert.DoesNotContain(turns, t => t.Text.Contains(gone));
    }

    [Fact]
    public void Codex_PlaceholderWordsSentAsAPromptStayAUserTurn()
    {
        var turns = ScreenTransformers.Codex("› Explain this codebase\n\n• It is a hub.\n\n› Ask Codex to do anything\n");
        Assert.Equal(new[] { ("user", "Explain this codebase"), ("assistant", "It is a hub.") }, turns.Select(t => (t.Role, t.Text)));
    }

    [Fact]
    public void Agy_ReplyIsItsOwnTurnAndTheBannerIsGone()
    {
        var turns = ScreenTransformers.Agy(Fixture("agy.screen.txt"));
        Assert.Equal(new[]
        {
            ("user", "/model"), ("assistant", "Model set to Gemini 3.8 Flash (Low)"),
            ("user", "hi"), ("assistant", "Hello! How can I help you today?"),
        }, turns.Select(t => (t.Role, t.Text)));
        Assert.All(turns, t => Assert.Equal("text", t.Kind));
        foreach (var gone in new[] { "Antigravity", "@", "▄", "▀", "for shortcuts", "─" })
            Assert.DoesNotContain(turns, t => t.Text.Contains(gone));
    }

    [Fact]
    public void GenericShell_CommandsBecomeBlocksWithColouredOutput()
    {
        var turns = ScreenTransformers.Generic(Fixture("generic-shell.screen.txt"));
        Assert.Equal(new[] { "command", "output", "command", "output" }, turns.Select(t => t.Kind));
        Assert.Equal("cargo test -p klark-link files", turns[0].Text);
        Assert.Equal("git status --short", turns[2].Text);
        // The idle prompt at the bottom is not a turn.
        Assert.DoesNotContain(turns, t => t.Kind == "command" && t.Text.Length == 0);

        var items = TranscriptFeeds.ScreenItems(turns, "box", "p1");
        var output = (MdAnsi)items[1].Md!.Single();
        Assert.Contains("<span class=\"af2\">ok</span>", output.Html);
        Assert.Contains("<span class=\"af1\">FAILED</span>", output.Html);
        Assert.Contains("<span class=\"af2 a-b\">   Compiling</span>", output.Html);
        Assert.False(output.Html.Contains('\u001b'));
        Assert.Equal(11, output.Lines);
        // Plain text (for search and the sidebar preview) has no escapes.
        Assert.False(items[1].Text!.Contains('\u001b'));
        // A path in the output links to the viewer.
        Assert.Contains("class=\"fpath\"", ((MdAnsi)items[3].Md!.Single()).Html);
        var rows = ChatRowModel.Build(items);
        Assert.Equal(new[] { "block", "block" }, rows.Select(r => r.Kind));
    }

    [Fact]
    public void GenericAgent_ReflowsProseKeepsListsAndDiffs()
    {
        var turns = ScreenTransformers.Generic(Fixture("generic-agent.screen.txt"));
        var user = turns.Single(t => t.Role == "user");
        Assert.Equal("add a retry to the upload", user.Text);
        var answer = turns.Last().Text;
        // The pane's wrap is undone: one paragraph, one line.
        Assert.Contains("exponential backoff so that a transient network error does not fail the whole offer, and log each attempt so the operator can see what happened when the hub was briefly unreachable from the machine.", answer);
        Assert.Contains("- retries: 3\n- backoff: 250ms doubling", answer);
        Assert.Contains("```\n@@ -150,3 +150,7 @@\n-    let resp", answer);
        Assert.DoesNotContain("────", answer);
    }

    [Fact]
    public void Ansi_EscapesHtmlAndKeepsColours()
    {
        Assert.Equal("&lt;b&gt;<span class=\"af1\">x</span>", AnsiHtml.Render("<b>\u001b[31mx\u001b[0m"));
        Assert.Equal("<span style=\"color:rgb(255,0,0)\">t</span>", AnsiHtml.Render("\u001b[38;2;255;0;0mt\u001b[m"));
        Assert.Equal("<span style=\"color:rgb(255,0,0)\">c</span>", AnsiHtml.Render("\u001b[38;5;196mc"));
        // Cursor movement and OSC titles are dropped, never shown.
        Assert.Equal("ab", AnsiHtml.Render("a\u001b[2K\u001b]0;title\u0007b"));
        // A style value can only ever be numbers.
        Assert.DoesNotContain("\"", AnsiHtml.Render("\u001b[38;2;1;2;3\"m").Replace("style=\"", "").Replace("\">", ""));
    }

    [Fact]
    public void ToolRows_ReadLikeClaude()
    {
        Assert.Equal("Read 3 files · Ran 2 commands", ChatRow.Phrase(["Read", "Bash", "Read", "Read", "Bash"], 0));
        Assert.Equal("Edited 1 file · thought", ChatRow.Phrase(["Edit"], 1));
        Assert.Equal("Searched 2 patterns", ChatRow.Phrase(["Grep", "Glob"], 0));
    }

    [Fact]
    public void Sidebar_FolderNamesAgentsAndSearch()
    {
        Assert.Equal("site", Sidebar.FolderName("/Users/me/work/agents/site/"));
        Assert.Equal("~", Sidebar.FolderName("/Users/me"));
        Assert.Equal("~", Sidebar.FolderName(""));
        var claude = new PaneInfo("p1", "w1", "t1", "", "claude", "working", "/Users/me/work/klark", false);
        var shell = new PaneInfo("p2", "w1", "t1", "zsh", "", "unknown", "/Users/me/work/klark", false);
        var empty = new PaneInfo("p3", "w1", "t1", "", "", "", "/Users/me", false);
        Assert.True(Sidebar.IsAgent(claude));
        Assert.False(Sidebar.IsAgent(shell));
        Assert.False(Sidebar.IsAgent(empty));
        // AND across words, over machine, session, agent, folder and the full path.
        Assert.True(Sidebar.Matches("mac", "main", claude, ["klark", "claude"]));
        Assert.True(Sidebar.Matches("mac", "main", claude, ["work/klark"]));
        Assert.True(Sidebar.Matches("mac", "main", claude, ["mac", "main"]));
        Assert.False(Sidebar.Matches("mac", "main", claude, ["klark", "devin"]));
        Assert.Equal("claude", Sidebar.AgentKey("Claude Code"));
        Assert.Equal("shell", Sidebar.AgentKey(""));
    }

    [Fact]
    public void FeedSearch_FindsMessageTextWithASnippet()
    {
        var feeds = new TranscriptFeeds(null!, Microsoft.Extensions.Logging.Abstractions.NullLogger<TranscriptFeeds>.Instance);
        var f = feeds.Get("box", "p1");
        TranscriptFeeds.SetScreen(f, Fixture("claude-table.screen.txt"));
        Assert.Equal("claude", f.Reader);
        var hits = feeds.Search("websocket removed", _ => true);
        var h = Assert.Single(hits);
        Assert.Equal(("box", "p1"), (h.MachineId, h.PaneId));
        Assert.Equal("WebSocket", h.Match);
        Assert.Empty(feeds.Search("websocket", _ => false));
        Assert.Equal("Here is how they compare:", TranscriptFeeds.Preview(f));
    }

    [Fact]
    public void Agy_CodeKeepsItsIndentationInFencedBlocks()
    {
        var turns = ScreenTransformers.Agy(Fixture("agy-code.screen.txt"));
        Assert.Equal(2, turns.Count);
        var reply = turns[1].Text;
        Assert.Contains("```python\ndef is_prime(n: int) -> bool:\n    \"\"\"Return True if n is a prime number, else False.\"\"\"\n    if n < 2:\n        return False\n    for i in range(2, int(n**0.5) + 1):\n        if n % i == 0:\n            return False\n    return True\n```", reply);
        Assert.Contains("```\nnumbers = [10, 15, 29, 44, 97, 88, 31, 100]", reply.Replace("```python", "```"));
        Assert.Contains("    # Output: 97".TrimStart(), reply);
        Assert.Contains("### 2. Find the Largest Prime in a Given List", reply);
        Assert.Contains("This function tests divisibility", reply);
        Assert.DoesNotContain("for shortcuts", reply);
        Assert.DoesNotContain("Gemini 3.8", reply);
        // The heading and sentences stay outside any fence.
        var outside = reply.Split("```").Where((_, n) => n % 2 == 0).ToList();
        Assert.Contains(outside, seg => seg.Contains("### 2. Find the Largest Prime"));
        Assert.Contains(outside, seg => seg.Contains("This function tests divisibility"));
        Assert.DoesNotContain(outside, seg => seg.Contains("print("));
    }
}

public class DevinQuestionTests
{
    [Xunit.Fact]
    public void DevinPickerOptionsBecomeButtons()
    {
        var screen = "── File · Destination ──\n Which MP3 should I send?\n ❯ 1 ganesha-speech.mp3\n     1.5 MB, Aug 12\n · 2 kathir_clean.mp3\n     2.5 MB, Jul 26\n ·   Other (type your own)\n\n↑↓ navigate · ⏎select · esc cancel";
        var b = KlarkHub.Services.TranscriptClean.Blocked(screen);
        Xunit.Assert.Equal(2, b.Options.Count);
        Xunit.Assert.Equal("1. ganesha-speech.mp3", b.Options[0].Label);
        Xunit.Assert.Contains("Which MP3", b.Question);
    }
}
