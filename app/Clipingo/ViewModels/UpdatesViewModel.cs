using System.IO;
using System.Net.Http;
using System.Text.Json;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>Settings → Updates, and the update bar in the main window (one instance for both).</summary>
public partial class UpdatesViewModel : ObservableObject
{
    private static readonly string SettingsFile =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Clipingo", "app-settings.json");

    public UpdatesViewModel()
    {
        try
        {
            if (File.Exists(SettingsFile))
                AutoCheck = JsonDocument.Parse(File.ReadAllText(SettingsFile)).RootElement.GetProperty("auto_update").GetBoolean();
        }
        catch (Exception ex) when (ex is IOException or JsonException or KeyNotFoundException or InvalidOperationException)
        {
        }
    }

    /// <summary>Raised when the installer has started: the app should close so it can be replaced.</summary>
    public event Action? ReadyToQuit;

    public string CurrentVersion => $"Clipingo {Updater.Current}";

    [ObservableProperty]
    public partial bool AutoCheck { get; set; } = true;

    [ObservableProperty]
    public partial string Status { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsAvailable), nameof(AvailableText))]
    [NotifyCanExecuteChangedFor(nameof(InstallCommand))]
    public partial UpdateInfo? Available { get; set; }

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(CheckCommand), nameof(InstallCommand))]
    public partial bool IsBusy { get; set; }

    [ObservableProperty]
    public partial double Progress { get; set; }

    public bool IsAvailable => Available is not null;
    public string AvailableText => Available is { } a ? $"Clipingo {a.Version} is available (you have {Updater.Current})." : "";

    partial void OnAutoCheckChanged(bool value)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(SettingsFile)!);
            File.WriteAllText(SettingsFile, JsonSerializer.Serialize(new Dictionary<string, bool> { ["auto_update"] = value }));
        }
        catch (IOException)
        {
        }
    }

    /// <summary>At start-up: a quiet check when automatic updates are on (no message when offline).</summary>
    public async Task StartupCheckAsync()
    {
        if (!AutoCheck)
            return;
        try
        {
            Available = await Updater.CheckAsync();
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or JsonException or IOException or KeyNotFoundException or InvalidOperationException)
        {
        }
    }

    private bool CanCheck() => !IsBusy;

    [RelayCommand(CanExecute = nameof(CanCheck))]
    private async Task CheckAsync()
    {
        IsBusy = true;
        Status = "Checking GitHub for a newer release…";
        try
        {
            Available = await Updater.CheckAsync();
            Status = Available is null ? $"You have the latest version ({Updater.Current})." : "";
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or JsonException or IOException or KeyNotFoundException or InvalidOperationException)
        {
            Status = $"Could not check for updates: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }

    private bool CanInstall() => Available is not null && !IsBusy;

    [RelayCommand(CanExecute = nameof(CanInstall))]
    private async Task InstallAsync()
    {
        IsBusy = true;
        Status = "Downloading the update…";
        try
        {
            await Updater.InstallAsync(Available!, new Progress<double>(p => Progress = p));
            Status = "Installing. Clipingo restarts when it is done.";
            ReadyToQuit?.Invoke();
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or IOException or InvalidDataException or System.ComponentModel.Win32Exception)
        {
            Status = $"Update failed: {ex.Message}";
        }
        finally
        {
            IsBusy = false;
        }
    }
}
