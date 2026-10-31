using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>Settings → System check: which tools and GPU backends the core can use.</summary>
public partial class SettingsViewModel(CoreClient core, Func<CoreClient> connect) : ObservableObject
{
    public ObservableCollection<ToolStatus> Tools { get; } = [];

    /// <summary>whisper.cpp runtime first, then the English / Hinglish / Hindi models.</summary>
    public ObservableCollection<SpeechModelItem> SpeechModels { get; } = [];

    public ObservableCollection<EncoderStatus> Encoders { get; } = [];

    [ObservableProperty]
    public partial string GpuSummary { get; set; } = "";

    [ObservableProperty]
    public partial string EncoderSummary { get; set; } = "";

    /// <summary>"Update your GPU driver…" advice from the encoder probe, if any.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasDriverHint))]
    public partial string? DriverHint { get; set; }

    public bool HasDriverHint => !string.IsNullOrEmpty(DriverHint);

    [ObservableProperty]
    public partial string Summary { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasError))]
    public partial string? ErrorMessage { get; set; }

    public bool HasError => !string.IsNullOrEmpty(ErrorMessage);

    /// <summary>Keeps existing items (an install may be running) and only refreshes their installed state.</summary>
    private void UpdateSpeechModels(SystemReport report)
    {
        bool Found(string tool) => report.Tools.Any(t => t.Name == tool && t.Found);
        string? Version(string tool) => report.Tools.FirstOrDefault(t => t.Name == tool && t.Found)?.Version;
        // Nothing heavy ships with the installer: tools and models download here, on demand.
        var rows = new List<(string Id, string Label, string Detail, bool Installed)>
        {
            ("ffmpeg", "FFmpeg", Version("ffmpeg") ?? "Video engine: cutting, captions, encoding · about 100 MB", Found("ffmpeg") && Found("ffprobe")),
            ("yt-dlp", "yt-dlp", Version("yt-dlp") ?? "Downloads YouTube videos and streams · about 18 MB", Found("yt-dlp")),
            ("deno", "Deno", Version("deno") ?? "Solves YouTube's player checks for yt-dlp · about 45 MB", Found("deno")),
            ("runtime", "whisper.cpp runtime", report.WhisperCli ?? "Official whisper.cpp build with NVIDIA CUDA (runs on CPU too) · 685 MB", report.WhisperCli is not null),
        };
        rows.AddRange(report.SpeechModels.Select(m => (m.Id, m.Label,
            m.Path ?? $"{m.DownloadMb:N0} MB download" + (m.NeedsConversion ? " · converted once on this PC (needs uv)" : ""),
            m.Installed)));
        foreach (var row in rows)
        {
            if (SpeechModels.FirstOrDefault(s => s.Id == row.Id) is { } existing)
                existing.Installed = row.Installed;
            else
                SpeechModels.Add(new SpeechModelItem(row.Id, row.Label, row.Detail, row.Installed, connect));
        }
    }

    [RelayCommand]
    private async Task CheckAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        try
        {
            var report = await core.CallAsync<SystemReport>("system.check", null, ct);
            Tools.Clear();
            foreach (var tool in report.Tools)
                Tools.Add(tool);
            GpuSummary = report.NvidiaGpus.Count > 0
                ? "NVIDIA: " + string.Join(", ", report.NvidiaGpus)
                : "No NVIDIA GPU found. DirectML / CPU backends will be used.";
            UpdateSpeechModels(report);
            Encoders.Clear();
            foreach (var encoder in report.Encoders)
                Encoders.Add(encoder);
            var selected = report.Encoders.FirstOrDefault(e => e.Name == report.SelectedEncoder);
            EncoderSummary = selected is null
                ? "No working video encoder. Install ffmpeg and run the check again."
                : $"Video encoder: {selected.Label} ({selected.Name})";
            DriverHint = report.Encoders.Select(e => e.Hint).FirstOrDefault(h => !string.IsNullOrEmpty(h));
            var missing = report.Tools.Where(t => t.Required && !t.Found).Select(t => t.Name).ToList();
            Summary = missing.Count == 0 ? "Everything required is installed." : "Missing required tools: " + string.Join(", ", missing);
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
