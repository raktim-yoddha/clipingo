using System.ComponentModel;
using Clipingo.Models;
using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Navigation;
using Windows.Media.Core;
using Windows.Media.Playback;
using VirtualKey = Windows.System.VirtualKey;
using Windows.UI.Core;

namespace Clipingo.Views;

/// <summary>What Jobs and Library pass when they open a clip in the editor.</summary>
public sealed record EditorTarget(string RunId, int? Rank);

/// <summary>
/// Editor view: wires the media player to the view model (playhead, Play clip stopping at the out point,
/// source vs exported preview). Editing itself lives in <see cref="EditorViewModel"/>.
/// </summary>
public sealed partial class EditorPage : Page
{
    public EditorViewModel ViewModel { get; } = App.Services.GetRequiredService<EditorViewModel>();

    private readonly MediaPlayer _media = new() { AutoPlay = false };
    private readonly DispatcherQueueTimer _clock;
    private string? _loaded;
    private bool _syncing;
    private bool _playingClip;

    public EditorPage()
    {
        InitializeComponent();
        Player.SetMediaPlayer(_media);
        _clock = DispatcherQueue.CreateTimer();
        _clock.Interval = TimeSpan.FromMilliseconds(100);
        _clock.Tick += (_, _) => Tick();
    }

    private bool ShowingSource => PreviewBar.SelectedItem != ExportTab;

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel.PropertyChanged += ViewModel_PropertyChanged;
        _clock.Start();
        if (e.Parameter is EditorTarget t)
            await ViewModel.OpenAsync(t.RunId, t.Rank);
        else
            await ViewModel.LoadRunsAsync();
        LoadPreview();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        ViewModel.PropertyChanged -= ViewModel_PropertyChanged;
        _clock.Stop();
        _media.Pause();
    }

    private void ViewModel_PropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(EditorViewModel.MediaPath) or nameof(EditorViewModel.ExportedVideo):
                LoadPreview();
                break;
            case nameof(EditorViewModel.Selected):
                LoadPreview();
                Seek(ViewModel.Start);
                break;
            case nameof(EditorViewModel.Start) when ShowingSource:
                Seek(ViewModel.Start);
                break;
            case nameof(EditorViewModel.Position) when !_syncing && ShowingSource:
                // Moved by a shortcut or the transcript, not by playback.
                _playingClip = false;
                Seek(ViewModel.Position);
                break;
            case nameof(EditorViewModel.Words):
                ShowTranscript();
                break;
        }
    }

    /// <summary>The trim's words as links: clicking one moves the playhead to it.</summary>
    private void ShowTranscript()
    {
        var paragraph = new Paragraph();
        foreach (var w in ViewModel.Words)
        {
            var link = new Hyperlink { UnderlineStyle = UnderlineStyle.None, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextFillColorPrimaryBrush"] };
            link.Inlines.Add(new Run { Text = w.W });
            var at = w.S;
            link.Click += (_, _) => ViewModel.SeekTo(at);
            paragraph.Inlines.Add(link);
            paragraph.Inlines.Add(new Run { Text = " " });
        }
        Transcript.Blocks.Clear();
        Transcript.Blocks.Add(paragraph);
    }

    /// <summary>Source video, or this clip's export, in the player.</summary>
    private void LoadPreview()
    {
        var path = ShowingSource ? ViewModel.MediaPath : ViewModel.ExportedVideo;
        if (path == _loaded)
            return;
        _loaded = path;
        _playingClip = false;
        _media.Source = path is null || !System.IO.File.Exists(path) ? null : MediaSource.CreateFromUri(new Uri(path));
        if (ShowingSource)
            Seek(ViewModel.Start);
    }

    private void Seek(double seconds)
    {
        _media.PlaybackSession.Position = TimeSpan.FromSeconds(Math.Max(0, seconds));
        _syncing = true;
        ViewModel.Position = seconds;
        Timeline.Value = Math.Clamp(seconds, Timeline.Minimum, Timeline.Maximum);
        _syncing = false;
    }

    private void Tick()
    {
        if (!ShowingSource || _media.Source is null)
            return;
        var now = _media.PlaybackSession.Position.TotalSeconds;
        if (_playingClip && now >= ViewModel.End)
        {
            _media.Pause();
            _playingClip = false;
        }
        _syncing = true;
        ViewModel.Position = now;
        Timeline.Value = Math.Clamp(now, Timeline.Minimum, Timeline.Maximum);
        _syncing = false;
    }

    private void Timeline_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_syncing || !ShowingSource)
            return;
        _playingClip = false;
        Seek(e.NewValue);
    }

    private void PlayClip_Click(object sender, Microsoft.UI.Xaml.RoutedEventArgs e)
    {
        if (ShowingSource)
        {
            Seek(ViewModel.Start);
            _playingClip = true;
        }
        else
        {
            _media.PlaybackSession.Position = TimeSpan.Zero;
        }
        _media.Play();
    }

    private void PreviewBar_SelectionChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args)
    {
        _media.Pause();
        LoadPreview();
    }

    private void PreviewSpeed_SelectionChanged(object sender, SelectionChangedEventArgs e) =>
        _media.PlaybackSession.PlaybackRate = PreviewSpeed.SelectedIndex switch { 0 => 0.5, 2 => 1.5, 3 => 2, _ => 1 };

    private void TogglePlay()
    {
        if (_media.PlaybackSession.PlaybackState == MediaPlaybackState.Playing)
            _media.Pause();
        else
            _media.Play();
    }

    private async void DeleteClip_Click(object sender, RoutedEventArgs e)
    {
        if (ViewModel.Selected is not { } clip)
            return;
        var confirm = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Delete this clip?",
            Content = $"\u201c{clip.Title}\u201d is removed from this video's list. Files you already exported stay in the folder.",
            PrimaryButtonText = "Delete",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await confirm.ShowAsync() == ContentDialogResult.Primary)
            await ViewModel.DeleteClipAsync();
    }

    /// <summary>Editor shortcuts; ignored while typing in a text box.</summary>
    private void Page_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (!ViewModel.HasClip || FocusManager.GetFocusedElement(XamlRoot) is TextBox or ComboBox or RichEditBox)
            return;
        var ctrl = InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Control).HasFlag(CoreVirtualKeyStates.Down);
        System.Windows.Input.ICommand? command = (ctrl, e.Key) switch
        {
            (true, VirtualKey.Z) => ViewModel.UndoTrimCommand,
            (true, VirtualKey.Y) => ViewModel.RedoTrimCommand,
            (true, VirtualKey.S) => ViewModel.SaveCommand,
            (true, VirtualKey.E) => ViewModel.ExportCommand,
            (false, VirtualKey.I) => ViewModel.SetStartCommand,
            (false, VirtualKey.O) => ViewModel.SetEndCommand,
            (false, VirtualKey.N) => ViewModel.NewClipCommand,
            _ => null,
        };
        if (command is not null)
        {
            if (command.CanExecute(null))
                command.Execute(null);
        }
        else if (!ctrl && e.Key == VirtualKey.Space)
            TogglePlay();
        else if (!ctrl && e.Key is VirtualKey.Left or VirtualKey.Right)
            ViewModel.SeekTo(ViewModel.Position + (e.Key == VirtualKey.Left ? -1 : 1));
        else
            return;
        e.Handled = true;
    }

    private void Run_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (e.AddedItems.FirstOrDefault() is RunListing run && run != ViewModel.SelectedRun)
            ViewModel.SelectedRun = run;
    }

    private void Clip_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (e.AddedItems.FirstOrDefault() is EditorClip clip && clip != ViewModel.Selected)
            ViewModel.Selected = clip;
    }
}
