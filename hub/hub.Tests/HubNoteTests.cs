using System.Text.Json.Nodes;
using KlarkHub.Services;

namespace KlarkHub.Tests;

public class HubNoteTests
{
    [Fact]
    public void Append_AddsOneParenthesisedLineAndStripRemovesIt()
    {
        var sent = HubNote.Append("add a readme", HubNote.DefaultBody);
        Assert.StartsWith("add a readme (klark hub: when you create or change a file", sent);
        Assert.DoesNotContain('\n', sent);
        Assert.Equal("add a readme", HubNote.Strip(sent));
    }

    [Theory]
    [InlineData("/context")]
    [InlineData("  /model sonnet")]
    [InlineData("")]
    [InlineData("   ")]
    public void Append_LeavesSlashCommandsAndEmptyTextAlone(string text) =>
        Assert.Equal(text, HubNote.Append(text, HubNote.DefaultBody));

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("  ")]
    public void Append_IsOffWhenTheNoteIsEmpty(string? body) =>
        Assert.Equal("hi", HubNote.Append("hi", body));

    [Fact]
    public void Append_KeepsACustomNoteOnOneLineWithoutClosingParentheses()
    {
        var sent = HubNote.Append("hi", "give paths (absolute)\nplease");
        Assert.Equal("hi (klark hub: give paths [absolute] please)", sent);
        Assert.Equal("hi", HubNote.Strip(sent));
    }

    [Fact]
    public void Append_DoesNotAddTwice()
    {
        var once = HubNote.Append("hi", "x");
        Assert.Equal(once, HubNote.Append(once, "x"));
    }

    [Fact]
    public void Strip_OnlyRemovesTheNoteAtTheEnd()
    {
        Assert.Equal("see (klark hub: note) in the docs", HubNote.Strip("see (klark hub: note) in the docs"));
        Assert.Equal("", HubNote.Strip(null));
    }

    [Fact]
    public void TheChatHidesTheNote_ForStructuredAndScreenMessages()
    {
        var o = new JsonObject { ["role"] = "user", ["kind"] = "text", ["text"] = "fix it (klark hub: " + HubNote.DefaultBody + ")" };
        Assert.Equal("fix it", TranscriptFeeds.ToItem(o, 0)!.Text);

        var turns = new List<TranscriptClean.ScreenTurn> { new("user", "text", "fix it (klark hub: when you create or change a file,\nstate its absolute path.)") };
        Assert.Equal("fix it", TranscriptFeeds.ScreenItems(turns)[0].Text);
    }
}
