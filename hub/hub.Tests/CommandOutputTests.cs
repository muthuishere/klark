using KlarkHub.Services;

namespace KlarkHub.Tests;

public class CommandOutputTests
{
    private static readonly string Rule = new('─', 80);
    private static readonly string Composer = $"{Rule}\n❭ Ask Devin to build features, fix bugs, or work on your code\n{Rule}\nSWE-2 Flash Medium          Press Ctrl+O to view the full thinking trace";

    private const string Before = "❭ hi\n\nHi — what are we working on?\n\n";

    private static readonly string ContextScreen =
        "❭ hi\n\nHi — what are we working on?\n\n❭ /context\n\n" +
        "Context Usage\n" +
        "⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁   swe-2-flash-medium · 31.6k/262.1k tokens (12.1%)\n" +
        "⛁ ⛁ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶   ⛁ System prompt: 5.1k tokens (2.0%)\n" +
        "⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶   ⛁ Tools: 8.8k tokens (3.4%)\n" +
        "⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶   ⛁ Messages: 17.7k tokens (6.7%)\n" +
        "⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶   ⛶ Free: 230.5k (87.9%)\n" +
        "Note: Token counts are estimates scaled by character ratio.\n\n";

    [Fact]
    public void DevinContext_GivesTheGridAndNote_WithoutChromeOrEcho()
    {
        var o = CommandOutput.Extract(Before + Composer, ContextScreen + Composer, "/context");
        Assert.StartsWith("Context Usage", o);
        Assert.Contains("⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁   swe-2-flash-medium", o);
        Assert.Contains("⛶ Free: 230.5k (87.9%)", o);
        Assert.EndsWith("Note: Token counts are estimates scaled by character ratio.", o);
        Assert.DoesNotContain("Ask Devin to build features", o);
        Assert.DoesNotContain("SWE-2 Flash Medium", o);
        Assert.DoesNotContain("───", o);
        Assert.DoesNotContain("/context", o);
        Assert.DoesNotContain("what are we working on", o);
    }

    [Fact]
    public void NothingNew_IsEmpty()
    {
        Assert.Equal("", CommandOutput.Extract(Before + Composer, Before + Composer, "/context"));
    }

    [Fact]
    public void OnlyTheEcho_IsEmpty()
    {
        Assert.Equal("", CommandOutput.Extract(Before + Composer, Before + "❭ /clear\n" + Composer, "/clear"));
    }

    [Fact]
    public void RepeatedLine_CountsOnlyWhatGrew()
    {
        var before = "line a\nline b\n";
        var after = "line a\nline b\nline a\nline c\n";
        Assert.Equal("line a\nline c", CommandOutput.Extract(before, after, "/x"));
    }

    [Fact]
    public void AnsiIsStripped()
    {
        var o = CommandOutput.Extract("old\n", "old\n\u001b[1mBold\u001b[0m result\n", "/x");
        Assert.Equal("Bold result", o);
    }

    [Theory]
    [InlineData("/context", true)]
    [InlineData("/model gpt", true)]
    [InlineData("/", false)]
    [InlineData("hello /context", false)]
    [InlineData("/a\nb", false)]
    [InlineData("/usr/bin/ls", true)]
    public void IsSlashCommand(string text, bool expected) => Assert.Equal(expected, CommandOutput.IsSlashCommand(text));

    [Fact]
    public void CommandItems_ShowOnceAndSurviveARefresh()
    {
        var feeds = new TranscriptFeeds(null!, Microsoft.Extensions.Logging.Abstractions.NullLogger<TranscriptFeeds>.Instance);
        var feed = feeds.Get("m", "p");
        var t0 = DateTimeOffset.UtcNow;
        var old = new ChatItem("a", "user", "text", "hi", null, null, "", null, t0.AddSeconds(-5), null);
        feed.Items = [old];
        feeds.AddCommandItems(feed, "/context", null, t0, 1);
        feeds.AddCommandItems(feed, "/context", "Context Usage", t0, 1);
        Assert.Equal(["a", "cmd-1", "cmd-1-out"], feed.Items.Select(i => i.Id).ToArray());
        Assert.Contains("```text\nContext Usage\n```", feed.Items[2].Text);
        // A later real item lands after the command; a refresh keeps the command items.
        var later = new ChatItem("b", "assistant", "text", "ok", null, null, "", null, t0.AddSeconds(5), null);
        feed.Items = TranscriptFeeds.Merge(feed, [old, later]);
        Assert.Equal(["a", "cmd-1", "cmd-1-out", "b"], feed.Items.Select(i => i.Id).ToArray());
        feed.Items = TranscriptFeeds.Merge(feed, [old, later]);
        Assert.Equal(4, feed.Items.Count);
    }
}
