using KlarkHub.Components.Pages;
using KlarkHub.Services;

namespace KlarkHub.Tests;

public class ChatWindowTests
{
    [Theory]
    [InlineData(0, 0, 0)]
    [InlineData(40, 0, 40)]
    [InlineData(ChatWindow.Always, 0, ChatWindow.Always)]
    [InlineData(ChatWindow.Always + 1, ChatWindow.Always + 1 - ChatWindow.Tail, ChatWindow.Always + 1)]
    [InlineData(3000, 3000 - ChatWindow.Tail, 3000)]
    public void Initial_draws_a_short_chat_whole_and_the_newest_rows_of_a_long_one(int n, int lo, int hi) =>
        Assert.Equal((lo, hi), ChatWindow.Initial(n));

    [Theory]
    [InlineData(-5, 10, 100, 0, 10)]
    [InlineData(20, 500, 100, 20, 100)]
    [InlineData(90, 40, 100, 40, 40)]
    [InlineData(10, 20, 0, 0, 0)]
    public void Clamp_keeps_the_range_inside_the_rows(int lo, int hi, int n, int eLo, int eHi) =>
        Assert.Equal((eLo, eHi), ChatWindow.Clamp(lo, hi, n));

    [Fact]
    public void Follow_keeps_reaching_the_newest_row_when_it_already_did()
    {
        Assert.Equal((2000, 2010), ChatWindow.Follow(2000, 2005, 2005, 2010));
        Assert.Equal((0, 160), ChatWindow.Follow(0, 150, 150, 160)); // a once-whole chat keeps growing whole until chat.js trims it
    }

    [Fact]
    public void Follow_leaves_a_window_scrolled_away_from_the_end_alone()
    {
        Assert.Equal((1000, 1040), ChatWindow.Follow(1000, 1040, 2000, 2005));
    }

    [Fact]
    public void Follow_clamps_when_rows_shrink() =>
        Assert.Equal((10, 30), ChatWindow.Follow(10, 100, 100, 30));

    [Fact]
    public void AroundRow_draws_the_row_with_context_and_stays_in_range()
    {
        var (lo, hi) = ChatWindow.AroundRow(500, 3000);
        Assert.InRange(500, lo, hi - 1);
        Assert.True(lo < 500 && hi > 500 + 1);
        Assert.Equal(0, ChatWindow.AroundRow(0, 3000).Lo);
        var last = ChatWindow.AroundRow(2999, 3000);
        Assert.Equal(3000, last.Hi);
        Assert.InRange(2999, last.Lo, last.Hi - 1);
    }

    [Fact]
    public void Earlier_adds_a_page_above_without_moving_the_end()
    {
        Assert.Equal((850, 1000), ChatWindow.Earlier(1000, 1000, 150, 1000));
        Assert.Equal((0, 120), ChatWindow.Earlier(100, 120, 150, 500));
    }

    private static ChatRowModel Row(string kind, string? text = null, IReadOnlyList<MdSegment>? md = null, string id = "x") =>
        new(id, kind, new[] { new ChatItem(id, "assistant", kind, text, null, null, "", null, null, md) });

    [Fact]
    public void Estimate_grows_with_content_and_is_never_zero()
    {
        var shortUser = ChatWindow.Estimate(Row("user", "hi"));
        var longUser = ChatWindow.Estimate(Row("user", new string('a', 700)));
        Assert.True(shortUser > 0 && longUser > shortUser);

        var small = ChatWindow.Estimate(Row("text", "hi", ChatMarkdown.Render("hi")));
        var code = ChatWindow.Estimate(Row("text", null, new MdSegment[] { new MdCode("cs", string.Join("\n", Enumerable.Repeat("var a = 1;", 60))) }));
        Assert.True(code > small * 5);
        Assert.True(ChatWindow.Estimate(Row("tools")) > 0);
    }
}
