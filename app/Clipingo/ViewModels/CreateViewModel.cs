using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>Create wizard, step 1: choose a video (file or YouTube link) and show its card.</summary>
public partial class CreateViewModel(CoreClient core) : ObservableObject
{
    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(ProbeCommand))]
    public partial string SourceInput { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasSource), nameof(Details), nameof(Channel))]
    public partial SourceInfo? Source { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasError))]
    public partial string? ErrorMessage { get; set; }

    public bool HasSource => Source is not null;
    public bool HasError => !string.IsNullOrEmpty(ErrorMessage);
    public string Channel => Source?.Channel ?? (Source?.Kind == "file" ? "Local file" : "");

    /// <summary>"1:02:33 · 1920×1080 · 30 fps · 12,345 views · 27 Oct 2026 · Was live".</summary>
    public string Details
    {
        get
        {
            if (Source is not { } s)
                return "";
            var parts = new List<string>();
            if (s.DurationS is { } d)
                parts.Add(TimeSpan.FromSeconds(Math.Round(d)).ToString(d >= 3600 ? @"h\:mm\:ss" : @"m\:ss"));
            if (s is { Width: { } w, Height: { } h })
                parts.Add($"{w}×{h}");
            if (s.Fps is { } fps)
                parts.Add($"{fps:0.##} fps");
            if (s.ViewCount is { } views)
                parts.Add($"{views:N0} views");
            if (s.UploadDate is { Length: 8 } date && DateTime.TryParseExact(date, "yyyyMMdd", null, System.Globalization.DateTimeStyles.None, out var when))
                parts.Add(when.ToString("d MMM yyyy"));
            if (s.WasLive)
                parts.Add("Was live");
            return string.Join(" · ", parts);
        }
    }

    private bool CanProbe() => !string.IsNullOrWhiteSpace(SourceInput);

    [RelayCommand(CanExecute = nameof(CanProbe))]
    private async Task ProbeAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        Source = null;
        try
        {
            Source = await core.CallAsync<SourceInfo>("source.probe", new { input = SourceInput.Trim().Trim('"') }, ct);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }
}
