using Clipingo.Models;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Clipingo.ViewModels;

/// <summary>One clip candidate in the Create results, selectable for rendering.</summary>
public partial class CandidateItem(Candidate candidate) : ObservableObject
{
    public Candidate Candidate { get; } = candidate;
    public int Rank => Candidate.Rank;
    public string Title => Candidate.Title;
    public string Score => Candidate.Score.ToString();
    public string Reasons => string.Join(" · ", Candidate.Reasons);
    public string Range => $"{Clock(Candidate.Start)} – {Clock(Candidate.End)} ({Candidate.End - Candidate.Start:0}s)";

    [ObservableProperty]
    public partial bool IsSelected { get; set; } = true;

    private static string Clock(double s) => TimeSpan.FromSeconds(s).ToString(s >= 3600 ? @"h\:mm\:ss" : @"m\:ss");
}
