using System.Diagnostics;
using System.IO;
using System.Net.Http;
using System.Reflection;
using System.Security.Cryptography;
using System.Text.Json;

namespace Clipingo.Services;

/// <summary>A newer release on GitHub: its version, notes page and installer.</summary>
public sealed record UpdateInfo(Version Version, string Notes, string PageUrl, string InstallerUrl, string? Sha256);

/// <summary>
/// Updates from GitHub Releases: the latest release's <c>Clipingo-Setup-*.exe</c> asset is downloaded, its SHA-256
/// checked against the digest GitHub publishes, and run silently; the installer closes and restarts the app.
/// </summary>
public static class Updater
{
    public const string Repository = "raktim-yoddha/clipingo";

    /// <summary>Release feed; <c>CLIPINGO_UPDATE_FEED</c> (a URL or a local JSON file) overrides it for testing.</summary>
    private static string Feed =>
        Environment.GetEnvironmentVariable("CLIPINGO_UPDATE_FEED") is { Length: > 0 } feed
            ? feed
            : $"https://api.github.com/repos/{Repository}/releases/latest";

    private static readonly HttpClient Http = new()
    {
        DefaultRequestHeaders = { { "User-Agent", "Clipingo-Updater" }, { "Accept", "application/vnd.github+json" } },
    };

    public static Version Current { get; } =
        Assembly.GetExecutingAssembly().GetName().Version is { } v ? new Version(v.Major, v.Minor, v.Build) : new Version(0, 0, 0);

    /// <summary>The newer release, or null when this is the latest. Throws when GitHub cannot be reached.</summary>
    public static async Task<UpdateInfo?> CheckAsync(CancellationToken ct = default)
    {
        string json;
        if (File.Exists(Feed))
            json = await File.ReadAllTextAsync(Feed, ct);
        else
        {
            using var response = await Http.GetAsync(Feed, ct);
            if (!response.IsSuccessStatusCode)
                throw new HttpRequestException($"GitHub answered {(int)response.StatusCode} for the latest release of {Repository}.");
            json = await response.Content.ReadAsStringAsync(ct);
        }
        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;
        if (!TryParseVersion(root.GetProperty("tag_name").GetString(), out var latest) || latest <= Current)
            return null;
        foreach (var asset in root.GetProperty("assets").EnumerateArray())
        {
            var name = asset.GetProperty("name").GetString() ?? "";
            if (!name.StartsWith("Clipingo-Setup", StringComparison.OrdinalIgnoreCase) || !name.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
                continue;
            var digest = asset.TryGetProperty("digest", out var d) ? d.GetString() : null;
            return new UpdateInfo(
                latest,
                root.TryGetProperty("body", out var body) ? body.GetString() ?? "" : "",
                root.TryGetProperty("html_url", out var page) ? page.GetString() ?? "" : "",
                asset.GetProperty("browser_download_url").GetString()!,
                digest is not null && digest.StartsWith("sha256:", StringComparison.Ordinal) ? digest[7..] : null);
        }
        return null;
    }

    /// <summary>"v1.2.3" / "1.2" → version; false for anything else.</summary>
    public static bool TryParseVersion(string? tag, out Version version)
    {
        version = new Version(0, 0, 0);
        if (tag is null || !Version.TryParse(tag.TrimStart('v', 'V'), out var v))
            return false;
        version = new Version(v.Major, v.Minor, Math.Max(0, v.Build));
        return true;
    }

    /// <summary>Downloads the installer, checks it, starts it silently and returns; the caller then quits.</summary>
    public static async Task InstallAsync(UpdateInfo update, IProgress<double> progress, CancellationToken ct = default)
    {
        var target = Path.Combine(Path.GetTempPath(), $"Clipingo-Setup-{update.Version}.exe");
        if (File.Exists(update.InstallerUrl))
            File.Copy(update.InstallerUrl, target, overwrite: true);
        else
        {
            using var response = await Http.GetAsync(update.InstallerUrl, HttpCompletionOption.ResponseHeadersRead, ct);
            response.EnsureSuccessStatusCode();
            var total = response.Content.Headers.ContentLength ?? 0;
            await using var source = await response.Content.ReadAsStreamAsync(ct);
            await using (var file = File.Create(target))
            {
                var buffer = new byte[1 << 16];
                long done = 0;
                int read;
                while ((read = await source.ReadAsync(buffer, ct)) > 0)
                {
                    await file.WriteAsync(buffer.AsMemory(0, read), ct);
                    done += read;
                    if (total > 0)
                        progress.Report(done * 100.0 / total);
                }
            }
        }
        if (update.Sha256 is { } expected)
        {
            await using var check = File.OpenRead(target);
            var actual = Convert.ToHexStringLower(await SHA256.HashDataAsync(check, ct));
            if (!actual.Equals(expected, StringComparison.OrdinalIgnoreCase))
            {
                File.Delete(target);
                throw new InvalidDataException("The downloaded update does not match the release's checksum; it was deleted.");
            }
        }
        // Inno Setup: no questions, close Clipingo, start it again when done.
        Process.Start(new ProcessStartInfo(target, "/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /RESTARTAPPLICATIONS") { UseShellExecute = true });
    }
}
