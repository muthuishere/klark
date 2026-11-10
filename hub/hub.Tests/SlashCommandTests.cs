using System.Text.Json.Nodes;
using KlarkHub.Services;

namespace KlarkHub.Tests;

/// <summary>A slash command's output is the answer to the command: the chat shows it.</summary>
public class SlashCommandTests
{
    private static JsonObject User(string text) => new() { ["role"] = "user", ["kind"] = "text", ["text"] = text };

    [Fact]
    public void LocalCommandOutput_IsShownAsTheAgentsCodeBlock()
    {
        var item = TranscriptFeeds.ToItem(User("<local-command-stdout>Context Usage\n  ⛁ Skills: 8.6k tokens (0.9%)\n  ⛶ Free space: 915.3k</local-command-stdout>"), 7);
        Assert.NotNull(item);
        Assert.Equal("assistant", item!.Role);
        Assert.StartsWith("```text\nContext Usage", item.Text);
        Assert.Contains("⛁ Skills: 8.6k tokens", item.Text);
        Assert.NotNull(item.Md);
    }

    [Fact]
    public void LocalCommandOutput_RemovesAnsiAndKeepsStderr()
    {
        var item = TranscriptFeeds.ToItem(User("<local-command-stderr>\u001b[31mUnknown command: /nope\u001b[0m</local-command-stderr>"), 1);
        Assert.Equal("assistant", item!.Role);
        Assert.Contains("Unknown command: /nope", item.Text);
        Assert.Equal("```text\nUnknown command: /nope\n```", item.Text!.Replace("\u001b", "<ESC>"));
    }

    [Fact]
    public void EmptyLocalCommandOutput_ShowsNothing() =>
        Assert.Null(TranscriptFeeds.ToItem(User("<local-command-stdout></local-command-stdout>"), 2));

    [Fact]
    public void TheCommandItself_StillShowsAsTheUsers()
    {
        var item = TranscriptFeeds.ToItem(User("<command-name>/context</command-name>\n<command-message>context</command-message>\n<command-args></command-args>"), 3);
        Assert.Equal(("user", "/context"), (item!.Role, item.Text));
    }

    [Fact]
    public void PlainUserText_IsUnchanged()
    {
        var item = TranscriptFeeds.ToItem(User("what is /context for?"), 4);
        Assert.Equal(("user", "what is /context for?"), (item!.Role, item.Text));
    }
}
