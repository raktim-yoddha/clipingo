using System.Text.Json.Nodes;
using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

public enum JobStatus { Queued, Running, NeedsReview, Rendering, Done, Failed, Cancelled }

/// <summary>What Create hands to Jobs: the video, the core's run and render options, and a one-line summary.</summary>
public sealed partial record JobRequest(string Source, string Title, bool Automatic, JsonObject RunOptions, JsonObject RenderOptions, string Summary);

/// <summary>One clipping job: find moments (run.create), then render all (Automatic) or the ones picked (Review &amp; edit).</summary>
public partial class JobItem(JobRequest request, Func<CoreClient> connect, SemaphoreSlim slots) : ObservableObject
{
    private readonly CancellationTokenSource _cts = new();

    public JobRequest Request { get; } = request;
    public string Title => Request.Title;
    public string Summary => Request.Summary;
    public string Workflow => Request.Automatic ? "Automatic" : "Review & edit";
    public DateTime Started { get; } = DateTime.Now;

    public ObservableCollection<CandidateItem> Candidates { get; } = [];
    public ObservableCollection<RenderedClip> Rendered { get; } = [];

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(StatusText), nameof(IsActive), nameof(NeedsReview), nameof(IsDone), nameof(IsFailed))]
    [NotifyCanExecuteChangedFor(nameof(CancelCommand), nameof(RenderSelectedCommand))]
    public partial JobStatus Status { get; set; } = JobStatus.Queued;

    [ObservableProperty]
    public partial string StageText { get; set; } = "Waiting for a free slot";

    /// <summary>0–100, or -1 while the current step has no measurable progress.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsIndeterminate))]
    public partial double ProgressValue { get; set; } = -1;

    [ObservableProperty]
    public partial string? ErrorMessage { get; set; }

    [ObservableProperty]
    public partial string ResultText { get; set; } = "";

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(OpenFolderCommand))]
    public partial string? OutputFolder { get; set; }

    public RunResult? Run { get; private set; }
    public bool IsIndeterminate => ProgressValue < 0;
    public bool IsActive => Status is JobStatus.Queued or JobStatus.Running or JobStatus.Rendering;
    public bool NeedsReview => Status == JobStatus.NeedsReview;
    public bool IsDone => Status == JobStatus.Done;
    public bool IsFailed => Status is JobStatus.Failed or JobStatus.Cancelled;

    public string StatusText => Status switch
    {
        JobStatus.Queued => "Queued",
        JobStatus.Running => "Finding moments",
        JobStatus.NeedsReview => "Pick your clips",
        JobStatus.Rendering => "Exporting",
        JobStatus.Done => "Done",
        JobStatus.Failed => "Failed",
        _ => "Cancelled",
    };

    private Progress<CoreProgress> TrackProgress() => new(p =>
    {
        StageText = p.Total is 100 ? $"{p.Stage} · {p.Done}%" : p.Total is > 0 ? $"{p.Stage} · {p.Done >> 20} / {p.Total.Value >> 20} MB" : p.Stage;
        ProgressValue = p.Total is > 0 ? p.Done * 100.0 / p.Total.Value : -1;
    });

    /// <summary>Runs the job to completion (or review). Never throws: failures end up in <see cref="ErrorMessage"/>.</summary>
    public async Task RunAsync()
    {
        try
        {
            await slots.WaitAsync(_cts.Token);
        }
        catch (OperationCanceledException)
        {
            Status = JobStatus.Cancelled;
            return;
        }
        try
        {
            Status = JobStatus.Running;
            StageText = "Starting";
            // Own connection per job: transcribing a stream takes minutes and must not block other calls.
            await using (var session = connect())
                Run = await session.CallAsync<RunResult>("run.create", new JsonObject { ["input"] = Request.Source, ["options"] = Request.RunOptions.DeepClone() }, _cts.Token, TrackProgress());
            foreach (var c in Run.Candidates)
                Candidates.Add(new CandidateItem(c));
            if (Candidates.Count == 0)
            {
                Finish(JobStatus.Done, "No matching moments. Try another length, or leave “What to clip” blank.");
                return;
            }
            if (!Request.Automatic)
            {
                Finish(JobStatus.NeedsReview, $"{Candidates.Count} moments found · {Run.Transcript.Words:N0} words with the {Run.Transcript.Model} model. Pick the clips to export.");
                return;
            }
            await RenderAsync(Candidates.Select(c => c.Rank).ToArray());
        }
        catch (OperationCanceledException)
        {
            Finish(JobStatus.Cancelled, "");
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
            Finish(JobStatus.Failed, "");
        }
        finally
        {
            slots.Release();
        }
    }

    private async Task RenderAsync(int[] ranks)
    {
        Status = JobStatus.Rendering;
        ResultText = "";
        await using var session = connect();
        var clips = await session.CallAsync<List<RenderedClip>>("run.render", new JsonObject { ["id"] = Run!.Id, ["ranks"] = CoreJson.Node(ranks), ["options"] = Request.RenderOptions.DeepClone() }, _cts.Token, TrackProgress());
        Rendered.Clear();
        foreach (var clip in clips)
            Rendered.Add(clip);
        OutputFolder = clips.Count > 0 ? Path.GetDirectoryName(clips[0].Video) : null;
        Finish(JobStatus.Done, $"{clips.Count} clip{(clips.Count == 1 ? "" : "s")} exported · each with an .srt and a Premiere .xml");
    }

    private void Finish(JobStatus status, string result)
    {
        Status = status;
        ResultText = result;
        StageText = "";
        ProgressValue = 0;
    }

    private bool CanRenderSelected() => Status == JobStatus.NeedsReview;

    [RelayCommand(CanExecute = nameof(CanRenderSelected))]
    private async Task RenderSelectedAsync()
    {
        var ranks = Candidates.Where(c => c.IsSelected).Select(c => c.Rank).ToArray();
        if (ranks.Length == 0)
        {
            ErrorMessage = "Select at least one clip.";
            return;
        }
        ErrorMessage = null;
        try
        {
            await RenderAsync(ranks);
        }
        catch (OperationCanceledException)
        {
            Finish(JobStatus.Cancelled, "");
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
            Finish(JobStatus.NeedsReview, ResultText);
        }
    }

    private bool CanCancel() => IsActive;

    [RelayCommand(CanExecute = nameof(CanCancel))]
    private void Cancel() => _cts.Cancel();

    private bool CanOpenFolder() => OutputFolder is not null;

    [RelayCommand(CanExecute = nameof(CanOpenFolder))]
    private async Task OpenFolderAsync()
    {
        if (OutputFolder is { } dir && !await Shell.RevealAsync(dir))
            ErrorMessage = $"Could not open {dir}";
    }
}

/// <summary>Every job of this session, newest first; two run at once (the rest wait for a slot).</summary>
public sealed partial class JobsService(Func<CoreClient> connect)
{
    public const int MaxParallel = 2;
    private readonly SemaphoreSlim _slots = new(MaxParallel, MaxParallel);

    public ObservableCollection<JobItem> Jobs { get; } = [];

    public int ActiveCount => Jobs.Count(j => j.IsActive);

    public JobItem Start(JobRequest request)
    {
        var job = new JobItem(request, connect, _slots);
        Jobs.Insert(0, job);
        _ = job.RunAsync();
        return job;
    }
}
