using System.IO;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>One installable piece: the whisper.cpp runtime or a speech model.</summary>
public partial class SpeechModelItem(string id, string label, string detail, bool installed, Func<CoreClient> connect) : ObservableObject
{
    public string Id { get; } = id;
    public string Label { get; } = label;
    public string Detail { get; } = detail;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(StateText), nameof(CanInstall))]
    public partial bool Installed { get; set; } = installed;

    [ObservableProperty]
    public partial string ProgressText { get; set; } = "";

    /// <summary>0–100, or -1 while the step has no known size.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsIndeterminate))]
    public partial double ProgressValue { get; set; }

    [ObservableProperty]
    public partial string? ErrorMessage { get; set; }

    public bool IsIndeterminate => ProgressValue < 0;
    public bool CanInstall => !Installed;
    public string StateText => Installed ? "Installed" : "Not installed";

    [RelayCommand]
    private async Task InstallAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        ProgressText = "Starting…";
        ProgressValue = -1;
        // Own connection: an install can take a long time and must not block other calls to clipd.
        await using var core = connect();
        var progress = new Progress<CoreProgress>(p =>
        {
            ProgressValue = p.Total is > 0 ? p.Done * 100.0 / p.Total.Value : -1;
            ProgressText = p.Total is > 0 ? $"{p.Stage}: {p.Done >> 20} / {p.Total.Value >> 20} MB" : p.Stage;
        });
        try
        {
            await core.CallAsync<System.Text.Json.JsonElement>("models.install", new { id = Id }, ct, progress);
            Installed = true;
            ProgressText = "";
        }
        catch (OperationCanceledException)
        {
            ProgressText = "Cancelled. Install again to resume the download.";
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
            ProgressText = "";
        }
    }
}
