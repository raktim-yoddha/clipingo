using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>Settings → System check: which tools and GPU backends the core can use.</summary>
public partial class SettingsViewModel(CoreClient core) : ObservableObject
{
    public ObservableCollection<ToolStatus> Tools { get; } = [];

    [ObservableProperty]
    public partial string GpuSummary { get; set; } = "";

    [ObservableProperty]
    public partial string Summary { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasError))]
    public partial string? ErrorMessage { get; set; }

    public bool HasError => !string.IsNullOrEmpty(ErrorMessage);

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
