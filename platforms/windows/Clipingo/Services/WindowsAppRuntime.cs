using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Windows.ApplicationModel.DynamicDependency;

namespace Clipingo.Services;

/// <summary>
/// Clipingo runs on Windows' shared Windows App Runtime instead of carrying its own copy, which keeps the installer
/// small (ADR 0006). A PC without it gets it once, from Microsoft, after the user says yes; other Windows apps share it.
/// </summary>
internal static partial class WindowsAppRuntime
{
    private static readonly uint MajorMinor = Microsoft.WindowsAppSDK.Release.MajorMinor;
    private static readonly string Version = $"{MajorMinor >> 16}.{MajorMinor & 0xFFFF}";
    private static readonly string Download = $"https://aka.ms/windowsappsdk/{Version}/latest/windowsappruntimeinstall-x64.exe";

    /// <summary>True when the runtime is ready; false when it is missing and could not be installed.</summary>
    public static bool Start()
    {
        if (TryStart())
            return true;

        var ask = $"Clipingo needs Microsoft's Windows App Runtime {Version}, which isn't on this PC yet.\n\n" +
            "Install it now? It is a one-time download from Microsoft (about 110 MB) that other Windows apps " +
            "share too. Clipingo starts by itself when it's done.";
        if (MessageBoxW(0, ask, "Clipingo", 0x41 /* MB_OKCANCEL | MB_ICONINFORMATION */) != 1)
            return false;

        try
        {
            Install();
        }
        catch (Exception e) when (e is HttpRequestException or IOException or Win32Exception or TaskCanceledException)
        {
        }

        if (TryStart())
            return true;

        MessageBoxW(0, $"The Windows App Runtime couldn't be installed. Get it from {Download}, then start Clipingo again.", "Clipingo", 0x30 /* MB_ICONWARNING */);
        return false;
    }

    private static bool TryStart() => Bootstrap.TryInitialize(MajorMinor, Microsoft.WindowsAppSDK.Release.VersionTag,
        new PackageVersion(Microsoft.WindowsAppSDK.Runtime.Version.UInt64), Bootstrap.InitializeOptions.None, out _);

    private static void Install()
    {
        var file = Path.Combine(Path.GetTempPath(), $"windowsappruntimeinstall-{Version}-x64.exe");
        using (var http = new HttpClient { Timeout = TimeSpan.FromMinutes(15) })
        using (var from = http.GetStreamAsync(new Uri(Download)).GetAwaiter().GetResult())
        using (var to = File.Create(file))
        {
            from.CopyTo(to);
        }

        using (var setup = Process.Start(new ProcessStartInfo(file, "--quiet") { UseShellExecute = false, CreateNoWindow = true }))
        {
            setup?.WaitForExit();
        }

        File.Delete(file);
    }

    [LibraryImport("user32.dll", StringMarshalling = StringMarshalling.Utf16)]
    private static partial int MessageBoxW(nint owner, string text, string caption, uint type);
}
