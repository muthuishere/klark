using KlarkHub.Services;

namespace KlarkHub.Tests;

/// <summary>On macOS /tmp is a link to /private/tmp: a pane there may be given either spelling.</summary>
public class FileLinksPrivateTmpTests
{
    [Theory]
    [InlineData("/tmp/hubtest/a.txt", "/private/tmp")]
    [InlineData("/private/tmp/hubtest/a.txt", "/tmp")]
    [InlineData("/private/tmp/a.txt", "/private/tmp")]
    [InlineData("/var/folders/x/a.txt", "/private/var/folders")]
    [InlineData("/private/var/folders/x/a.txt", "/var/folders")]
    public void SameFolderUnderEitherSpelling_IsAllowed(string path, string cwd) =>
        Assert.Null(FileLinks.Refusal(path, cwd));

    [Theory]
    [InlineData("/tmp/other/a.txt", "/private/tmp/hubtest")]
    [InlineData("/etc/hosts", "/private/tmp")]
    [InlineData("/privatetmp/a.txt", "/private/tmp")]
    [InlineData("/tmp/../etc/passwd", "/private/tmp")]
    public void OtherFolders_StillRefused(string path, string cwd) =>
        Assert.NotNull(FileLinks.Refusal(path, cwd));
}
