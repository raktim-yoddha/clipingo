using System.ComponentModel;
using Clipingo.Models;
using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Input;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;
using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.UI.Core;
using VirtualKey = Windows.System.VirtualKey;

namespace Clipingo.Views;

/// <summary>What Jobs and Library pass when they open a clip in the editor.</summary>
public sealed record EditorTarget(string RunId, int? Rank);

/// <summary>
/// Editor view: drives the player. In Edit it plays the timeline's parts one after another from the source video
/// (seeking from one part to the next), keeps the playhead in sync and draws a rough caption / title preview over
/// it; Source shows the whole video for marking parts; the export tabs play the exported files. Editing itself lives
/// in <see cref="EditorViewModel"/>, the timeline in <see cref="TimelineView"/>.
/// </summary>
public sealed partial class EditorPage : Page
{
    public EditorViewModel ViewModel { get; } = App.Services.GetRequiredService<EditorViewModel>();

    private readonly MediaPlayer _media = new() { AutoPlay = false };
    private readonly DispatcherQueueTimer _clock;
    private string? _loaded;
    private bool _syncing;
    private int _part;

    /// <summary>9:16 preview panels: muted players of the same video, cropped by placement inside a clipped canvas.</summary>
    private readonly List<(MediaPlayer Media, MediaPlayerElement View, Canvas Frame)> _panels = [];
    private readonly List<Microsoft.UI.Xaml.Shapes.Rectangle> _boxes = [];

    public EditorPage()
    {
        InitializeComponent();
        Player.SetMediaPlayer(_media);
        Timeline.Model = ViewModel;
        _clock = DispatcherQueue.CreateTimer();
        _clock.Interval = TimeSpan.FromMilliseconds(40);
        _clock.Tick += (_, _) => Tick();
        _media.PlaybackSession.PlaybackStateChanged += (_, _) => DispatcherQueue.TryEnqueue(() =>
        {
            if (!_left)
                UpdatePlayGlyph();
        });
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel.PropertyChanged += ViewModel_PropertyChanged;
        ViewModel.TimelineChanged += ViewModel_TimelineChanged;
        ViewModel.FramingChanged += UpdateFraming;
        _clock.Start();
        if (e.Parameter is EditorTarget t)
            await ViewModel.OpenAsync(t.RunId, t.Rank);
        else
            await ViewModel.LoadRunsAsync();
        if (_left)
            return;
        SyncPreviewBar();
        LoadPreview();
        Timeline.Fit();
    }

    private bool _left;

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        ViewModel.PropertyChanged -= ViewModel_PropertyChanged;
        ViewModel.TimelineChanged -= ViewModel_TimelineChanged;
        ViewModel.FramingChanged -= UpdateFraming;
        _clock.Stop();
        _ = ViewModel.SaveAsync();
        // The view model outlives this page: let go of it, and of the players, whose decoders hold well over 100 MB.
        _left = true;
        Bindings.StopTracking();
        Timeline.Model = null;
        Player.SetMediaPlayer(null);
        _media.Dispose();
        foreach (var p in _panels)
        {
            p.View.SetMediaPlayer(null);
            p.Media.Dispose();
        }
        _panels.Clear();
        PhoneCanvas.Children.Clear();
        // An idle app allocates too little for a collection to come by itself, and that is what frees the page's visuals.
        DispatcherQueue.TryEnqueue(() => GC.Collect());
    }

    private void ViewModel_PropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(EditorViewModel.MediaPath) or nameof(EditorViewModel.Exported):
                LoadPreview();
                break;
            case nameof(EditorViewModel.Selected):
                LoadPreview();
                DispatcherQueue.TryEnqueue(Timeline.Fit);
                break;
            case nameof(EditorViewModel.Preview):
                SyncPreviewBar();
                LoadPreview();
                break;
            case nameof(EditorViewModel.FormatIndex):
                LayoutStage();
                break;
            case nameof(EditorViewModel.Position) when !_syncing && ViewModel.IsEditPreview:
                SeekEdit(ViewModel.Position);
                UpdateFraming();
                break;
            case nameof(EditorViewModel.SourcePosition) when !_syncing && ViewModel.IsSourcePreview:
                _media.PlaybackSession.Position = TimeSpan.FromSeconds(ViewModel.SourcePosition);
                break;
            case nameof(EditorViewModel.CaptionPositionIndex) or nameof(EditorViewModel.CaptionSize) or nameof(EditorViewModel.CaptionStyleIndex)
                or nameof(EditorViewModel.BurnCaptions) or nameof(EditorViewModel.ShowTitle):
                UpdateOverlay();
                break;
        }
    }

    /// <summary>Parts changed (split, trim, move): keep playing from the same place on the edit's clock.</summary>
    private void ViewModel_TimelineChanged()
    {
        if (ViewModel.IsEditPreview)
            SeekEdit(ViewModel.Position);
    }

    // ── Preview ──

    private void SyncPreviewBar()
    {
        var item = ViewModel.Preview switch
        {
            PreviewMode.Source => SourceTab,
            PreviewMode.Vertical => VerticalTab,
            PreviewMode.Horizontal => HorizontalTab,
            _ => EditTab,
        };
        if (PreviewBar.SelectedItem != item)
            PreviewBar.SelectedItem = item;
    }

    private void PreviewBar_SelectionChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args)
    {
        _media.Pause();
        ViewModel.Preview = sender.SelectedItem == SourceTab ? PreviewMode.Source
            : sender.SelectedItem == VerticalTab ? PreviewMode.Vertical
            : sender.SelectedItem == HorizontalTab ? PreviewMode.Horizontal
            : PreviewMode.Edit;
    }

    /// <summary>Source video (Edit, Source) or one of the clip's exports in the player.</summary>
    private void LoadPreview()
    {
        var path = ViewModel.Preview switch
        {
            PreviewMode.Vertical => ViewModel.Selected?.Vertical,
            PreviewMode.Horizontal => ViewModel.Selected?.Horizontal,
            _ => ViewModel.HasClip ? ViewModel.MediaPath : null,
        };
        Player.AreTransportControlsEnabled = !ViewModel.IsEditPreview;
        if (path != _loaded)
        {
            _loaded = path;
            _media.Source = path is null || !System.IO.File.Exists(path) ? null : MediaSource.CreateFromUri(new Uri(path));
        }
        if (ViewModel.IsEditPreview)
            SeekEdit(ViewModel.Position);
        else if (ViewModel.IsSourcePreview)
            _media.PlaybackSession.Position = TimeSpan.FromSeconds(ViewModel.SourcePosition > 0 ? ViewModel.SourcePosition : ViewModel.Locate(ViewModel.Position).Source);
        else
            _media.PlaybackSession.Position = TimeSpan.Zero;
        LayoutStage();
    }

    /// <summary>Edit: the player to the source frame under the playhead.</summary>
    private void SeekEdit(double t)
    {
        var (i, src) = ViewModel.Locate(t);
        if (i < 0 || _media.Source is null)
            return;
        _part = i;
        _media.PlaybackSession.Position = TimeSpan.FromSeconds(src);
        foreach (var p in _panels)
            p.Media.PlaybackSession.Position = TimeSpan.FromSeconds(src);
    }

    private void Tick()
    {
        if (_media.Source is null)
            return;
        var session = _media.PlaybackSession;
        var now = session.Position.TotalSeconds;
        _syncing = true;
        if (ViewModel.IsEditPreview && ViewModel.Segments.Count > 0)
        {
            _part = Math.Clamp(_part, 0, ViewModel.Segments.Count - 1);
            var seg = ViewModel.Segments[_part];
            var playing = session.PlaybackState == MediaPlaybackState.Playing;
            if (playing && now >= seg.End - 0.02)
            {
                // Next part, or the end (or back to the start when looping).
                if (_part + 1 < ViewModel.Segments.Count)
                {
                    _part++;
                    session.Position = TimeSpan.FromSeconds(ViewModel.Segments[_part].Start);
                    now = ViewModel.Segments[_part].Start;
                }
                else if (LoopToggle.IsChecked == true)
                {
                    _part = 0;
                    session.Position = TimeSpan.FromSeconds(ViewModel.Segments[0].Start);
                    now = ViewModel.Segments[0].Start;
                }
                else
                {
                    _media.Pause();
                    now = seg.End;
                }
                seg = ViewModel.Segments[_part];
            }
            // Paused: the playhead leads (a seek may not have reached the player yet).
            if (playing)
                ViewModel.Position = ViewModel.StartOf(_part) + Math.Clamp(now - seg.Start, 0, seg.Length);
            UpdateCaption();
            if (VerticalEdit)
            {
                SyncPanels(now);
                if (playing)
                    UpdateFraming();
            }
        }
        else if (ViewModel.IsSourcePreview)
        {
            ViewModel.SourcePosition = now;
        }
        _syncing = false;
    }

    private void TogglePlay()
    {
        if (_media.PlaybackSession.PlaybackState == MediaPlaybackState.Playing)
        {
            _media.Pause();
            return;
        }
        // Edit at the end: play again from the start.
        if (ViewModel.IsEditPreview && ViewModel.Position >= ViewModel.Length - 0.05)
            ViewModel.Seek(0);
        _media.Play();
    }

    private void UpdatePlayGlyph()
    {
        var playing = _media.PlaybackSession.PlaybackState == MediaPlaybackState.Playing;
        PlayGlyph.Glyph = playing ? "" : "";
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(PlayButton, playing ? "Pause" : "Play");
    }

    private void Play_Click(object sender, RoutedEventArgs e) => TogglePlay();

    private void ToStart_Click(object sender, RoutedEventArgs e) => ViewModel.Seek(0);

    private void Back_Click(object sender, RoutedEventArgs e) => Step(-1);

    private void Forward_Click(object sender, RoutedEventArgs e) => Step(1);

    private void Step(double seconds)
    {
        if (ViewModel.IsEditPreview)
            ViewModel.Seek(ViewModel.Position + seconds);
        else
            _media.PlaybackSession.Position = TimeSpan.FromSeconds(Math.Max(0, _media.PlaybackSession.Position.TotalSeconds + seconds));
    }

    private void PreviewSpeed_SelectionChanged(object sender, SelectionChangedEventArgs e) =>
        _media.PlaybackSession.PlaybackRate = PreviewSpeed.SelectedIndex switch { 0 => 0.5, 2 => 1.5, 3 => 2, _ => 1 };

    // ── 9:16 framing preview ──

    private bool VerticalEdit => ViewModel.IsEditPreview && ViewModel.IsVertical && ViewModel.HasClip;

    /// <summary>One screen, or source + 9:16 frame side by side; the caption preview goes on the output.</summary>
    private void LayoutStage()
    {
        var vertical = VerticalEdit;
        Phone.Visibility = vertical ? Visibility.Visible : Visibility.Collapsed;
        if (vertical)
            Phone.Width = Math.Max(120, Math.Min(Stage.ActualHeight * 9 / 16, Stage.ActualWidth * 0.42));
        var host = vertical ? (Panel)Phone : SourceView;
        if (Overlay.Parent is Panel current && current != host)
        {
            current.Children.Remove(Overlay);
            host.Children.Add(Overlay);
        }
        if (!vertical)
        {
            foreach (var b in _boxes)
                b.Visibility = Visibility.Collapsed;
            foreach (var p in _panels)
            {
                p.Media.Pause();
                p.Frame.Visibility = Visibility.Collapsed;
            }
        }
        UpdateFraming();
        UpdateOverlay();
    }

    /// <summary>The source picture's rectangle inside the source view (letterboxed).</summary>
    private (double X, double Y, double W, double H) VideoRect()
    {
        var (sw, sh) = ViewModel.SourceSize;
        var (vw, vh) = (SourceView.ActualWidth, SourceView.ActualHeight);
        var scale = Math.Min(vw / sw, vh / sh);
        return ((vw - sw * scale) / 2, (vh - sh * scale) / 2, sw * scale, sh * scale);
    }

    /// <summary>The crop boxes and the 9:16 panels for the shot under the playhead.</summary>
    private void UpdateFraming()
    {
        if (!VerticalEdit || Phone.ActualWidth <= 0)
            return;
        var t = ViewModel.Position;
        var (sw, sh) = ViewModel.SourceSize;
        var shot = ViewModel.ShotAt(t);
        var boxed = shot is { Panels.Count: > 0 };
        // Whole frame (or not analysed yet): the picture across the middle of the frame.
        IReadOnlyList<FramingPanel> panels = boxed
            ? shot!.Panels
            : [new FramingPanel(sw, sh, [[t, 0, 0]], (int)((1920 - 1080.0 * sh / sw) / 2), (int)(1080.0 * sh / sw))];

        ShotChip.Visibility = ViewModel.ShotText.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        var (ox, oy, vw, _) = VideoRect();
        var k = vw / sw;
        var accent = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentFillColorDefaultBrush"];
        while (_boxes.Count < 2)
        {
            var r = new Microsoft.UI.Xaml.Shapes.Rectangle { Stroke = accent, StrokeThickness = 2, RadiusX = 3, RadiusY = 3 };
            _boxes.Add(r);
            BoxCanvas.Children.Add(r);
        }
        for (var i = 0; i < _boxes.Count; i++)
        {
            var box = _boxes[i];
            box.Visibility = boxed && i < panels.Count ? Visibility.Visible : Visibility.Collapsed;
            if (box.Visibility == Visibility.Collapsed)
                continue;
            var (x, y) = EditorViewModel.CropAt(panels[i], t);
            (box.Width, box.Height) = (panels[i].W * k, panels[i].H * k);
            Canvas.SetLeft(box, ox + x * k);
            Canvas.SetTop(box, oy + y * k);
        }

        // Output pixels → preview pixels; each panel's crop is scaled to the frame's width.
        var (pw, ph) = (Phone.ActualWidth, Phone.ActualHeight);
        var scaleOut = Math.Min(pw / 1080, ph / 1920);
        var (fw, top) = (1080 * scaleOut, (ph - 1920 * scaleOut) / 2);
        for (var i = 0; i < Math.Max(panels.Count, _panels.Count); i++)
        {
            if (i >= panels.Count)
            {
                _panels[i].Frame.Visibility = Visibility.Collapsed;
                _panels[i].Media.Pause();
                continue;
            }
            var view = PanelView(i);
            var p = panels[i];
            var (x, y) = EditorViewModel.CropAt(p, t);
            // Smart zoom enlarges around the face, 40 % down the frame, like the export.
            var z = shot is { } current && panels.Count == 1 ? EditorViewModel.ZoomAt(current, t) : 1;
            (x, y) = (x + p.W * (1 - 1 / z) * 0.5, y + p.H * (1 - 1 / z) * 0.4);
            var (sx, sy) = (fw / p.W * z, p.DestH * scaleOut / p.H * z);
            view.Frame.Visibility = Visibility.Visible;
            (view.Frame.Width, view.Frame.Height) = (fw, p.DestH * scaleOut);
            view.Frame.Clip = new Microsoft.UI.Xaml.Media.RectangleGeometry { Rect = new Windows.Foundation.Rect(0, 0, fw, p.DestH * scaleOut) };
            Canvas.SetLeft(view.Frame, (pw - fw) / 2);
            Canvas.SetTop(view.Frame, top + p.DestY * scaleOut);
            (view.View.Width, view.View.Height) = (sw * sx, sh * sy);
            Canvas.SetLeft(view.View, -x * sx);
            Canvas.SetTop(view.View, -y * sy);
        }
        PlaceAutoCaption();
    }

    /// <summary>Captions at the height the shot gives them, on the preview frame.</summary>
    private void PlaceAutoCaption()
    {
        if (!VerticalEdit || ViewModel.CaptionPositionIndex != 0 || ViewModel.ShotAt(ViewModel.Position) is not { } shot)
            return;
        var scale = Math.Min(Phone.ActualWidth / 1080, Phone.ActualHeight / 1920);
        var top = (Phone.ActualHeight - 1920 * scale) / 2;
        CaptionOverlay.VerticalAlignment = VerticalAlignment.Top;
        CaptionOverlay.Margin = new Thickness(12, Math.Max(0, top + shot.CaptionY * scale - CaptionOverlay.FontSize * 0.7), 12, 0);
    }

    /// <summary>The i-th panel's player, made on first use: muted, same video as the main player.</summary>
    private (MediaPlayer Media, MediaPlayerElement View, Canvas Frame) PanelView(int i)
    {
        while (_panels.Count <= i)
        {
            var media = new MediaPlayer { AutoPlay = false, IsMuted = true };
            var view = new MediaPlayerElement { Stretch = Microsoft.UI.Xaml.Media.Stretch.Fill, IsHitTestVisible = false };
            view.SetMediaPlayer(media);
            var frame = new Canvas();
            frame.Children.Add(view);
            PhoneCanvas.Children.Add(frame);
            _panels.Add((media, view, frame));
        }
        var panel = _panels[i];
        if (ViewModel.MediaPath is { } path && panel.View.Tag as string != path)
        {
            panel.View.Tag = path;
            panel.Media.Source = MediaSource.CreateFromUri(new Uri(path));
            panel.Media.PlaybackSession.Position = _media.PlaybackSession.Position;
        }
        return panel;
    }

    /// <summary>Panel players follow the main one: same play state, rate and position (within 0.12 s).</summary>
    private void SyncPanels(double now)
    {
        var playing = _media.PlaybackSession.PlaybackState == MediaPlaybackState.Playing;
        foreach (var (media, _, frame) in _panels)
        {
            if (frame.Visibility != Visibility.Visible || media.Source is null)
                continue;
            var session = media.PlaybackSession;
            session.PlaybackRate = _media.PlaybackSession.PlaybackRate;
            if (Math.Abs(session.Position.TotalSeconds - now) > 0.12)
                session.Position = TimeSpan.FromSeconds(now);
            var panelPlaying = session.PlaybackState == MediaPlaybackState.Playing;
            if (playing && !panelPlaying)
                media.Play();
            else if (!playing && panelPlaying)
                media.Pause();
        }
    }

    // ── Caption and title preview ──

    private void Stage_SizeChanged(object sender, SizeChangedEventArgs e) => LayoutStage();

    private void UpdateOverlay()
    {
        var edit = ViewModel.IsEditPreview && ViewModel.HasClip;
        TitleOverlay.Visibility = edit && ViewModel.ShowTitle ? Visibility.Visible : Visibility.Collapsed;
        CaptionOverlay.Visibility = edit && ViewModel.BurnCaptions ? Visibility.Visible : Visibility.Collapsed;
        CaptionOverlay.VerticalAlignment = ViewModel.CaptionPositionIndex switch { 2 => VerticalAlignment.Center, 3 => VerticalAlignment.Top, _ => VerticalAlignment.Bottom };
        // Sized to the output frame: the 9:16 preview, or the whole stage in 16:9.
        var vertical = VerticalEdit;
        CaptionOverlay.Margin = new Thickness(12, ViewModel.CaptionPositionIndex == 3 ? (vertical ? 72 : 80) : 32, 12, vertical ? 56 : 32);
        CaptionOverlay.FontSize = Math.Max(11, (vertical ? Phone.ActualWidth / 11 : Stage.ActualHeight / 16) * ViewModel.CaptionSize / 100);
        PlaceAutoCaption();
        if (ViewModel.Style is { } style)
            CaptionOverlay.FontFamily = new FontFamily(style.Font);
        _shownWord = -2;
        UpdateCaption();
    }

    private int _shownWord = -2;

    /// <summary>The 4-word group around the playhead, the current word highlighted (like the export's word pop).</summary>
    private void UpdateCaption()
    {
        // The title is a hook: the first 4 seconds, as in the export.
        TitleOverlay.Visibility = ViewModel.IsEditPreview && ViewModel.HasClip && ViewModel.ShowTitle && ViewModel.Position < 4 ? Visibility.Visible : Visibility.Collapsed;
        if (CaptionOverlay.Visibility != Visibility.Visible)
            return;
        var words = ViewModel.TimelineWords;
        var t = ViewModel.Position;
        var k = -1;
        for (var i = 0; i < words.Count && words[i].At <= t; i++)
            k = i;
        if (k >= 0 && t > words[k].Until + 0.6)
            k = -1; // a pause: no caption
        if (k == _shownWord)
            return;
        _shownWord = k;
        CaptionOverlay.Inlines.Clear();
        if (k < 0)
            return;
        var style = ViewModel.Style;
        var primary = style is null ? new SolidColorBrush(Microsoft.UI.Colors.White) : Format.Hex(style.Primary);
        var highlight = style is null ? (Brush)Application.Current.Resources["AccentFillColorDefaultBrush"] : Format.Hex(style.Highlight);
        var first = k / 4 * 4;
        for (var i = first; i < Math.Min(words.Count, first + 4); i++)
        {
            var text = style?.Upper == true ? words[i].Text.ToUpperInvariant() : words[i].Text;
            CaptionOverlay.Inlines.Add(new Run { Text = text + " ", Foreground = i == k ? highlight : primary });
        }
    }

    // ── Clips and toolbar ──

    private async void DeleteClip_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is EditorClip c)
            ViewModel.Selected = c;
        if (ViewModel.Selected is not { } clip)
            return;
        var confirm = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Delete this clip?",
            Content = $"“{clip.Title}” is removed from this video's list. Files you already exported stay in the folder.",
            PrimaryButtonText = "Delete",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await confirm.ShowAsync() == ContentDialogResult.Primary)
            await ViewModel.DeleteClipAsync();
    }

    private void PlayVertical_Click(object sender, RoutedEventArgs e) => PlayExport(sender, PreviewMode.Vertical);

    private void PlayHorizontal_Click(object sender, RoutedEventArgs e) => PlayExport(sender, PreviewMode.Horizontal);

    private void PlayExport(object sender, PreviewMode mode)
    {
        if ((sender as FrameworkElement)?.Tag is EditorClip clip)
            ViewModel.Selected = clip;
        ViewModel.Preview = mode;
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!_left)
                _media.Play();
        });
    }

    private void AddClipMenu_Opening(object sender, object e)
    {
        AddClipMenu.Items.Clear();
        foreach (var clip in ViewModel.Clips.Where(c => c != ViewModel.Selected))
            AddClipMenu.Items.Add(new MenuFlyoutItem { Text = $"{clip.Title}  ({clip.Info})", Command = ViewModel.InsertClipCommand, CommandParameter = clip });
        if (AddClipMenu.Items.Count == 0)
            AddClipMenu.Items.Add(new MenuFlyoutItem { Text = "No other clips in this video", IsEnabled = false });
        AddClipMenu.Items.Add(new MenuFlyoutSeparator());
        var source = new MenuFlyoutItem { Text = "Any part of the video…" };
        source.Click += (_, _) => ViewModel.Preview = PreviewMode.Source;
        AddClipMenu.Items.Add(source);
    }

    private void ClipFilter_SelectionChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args) =>
        ViewModel.ClipFilter = sender.Items.IndexOf(sender.SelectedItem);

    private void InspectorBar_SelectionChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args)
    {
        LayoutPanel.Visibility = sender.SelectedItem == LayoutTab ? Visibility.Visible : Visibility.Collapsed;
        CaptionsPanel.Visibility = sender.SelectedItem == CaptionsTab ? Visibility.Visible : Visibility.Collapsed;
        ClipPanel.Visibility = sender.SelectedItem == ClipTab ? Visibility.Visible : Visibility.Collapsed;
        InspectorScroll.ChangeView(null, 0, null, true);
    }

    private async void ShotOption_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is string layout)
            await ViewModel.ChooseLayoutAsync(layout);
    }

    private async void ShotAuto_Click(object sender, RoutedEventArgs e) => await ViewModel.ChooseLayoutAsync(null);

    private void ZoomIn_Click(object sender, RoutedEventArgs e) => Timeline.ZoomBy(1.5);

    private void ZoomOut_Click(object sender, RoutedEventArgs e) => Timeline.ZoomBy(1 / 1.5);

    private void Fit_Click(object sender, RoutedEventArgs e) => Timeline.Fit();

    /// <summary>Editor shortcuts; ignored while typing.</summary>
    private void Page_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        var focused = FocusManager.GetFocusedElement(XamlRoot);
        if (!ViewModel.HasClip || focused is TextBox or ComboBox or RichEditBox or AutoSuggestBox or Slider or RadioButton
            || (e.Key == VirtualKey.Space && focused is Microsoft.UI.Xaml.Controls.Primitives.ButtonBase or ToggleSwitch))
            return;
        bool Down(VirtualKey k) => InputKeyboardSource.GetKeyStateForCurrentThread(k).HasFlag(CoreVirtualKeyStates.Down);
        var (ctrl, shift) = (Down(VirtualKey.Control), Down(VirtualKey.Shift));
        System.Windows.Input.ICommand? command = (ctrl, e.Key) switch
        {
            (true, VirtualKey.Z) => ViewModel.UndoCommand,
            (true, VirtualKey.Y) => ViewModel.RedoCommand,
            (true, VirtualKey.E) => ViewModel.ExportCommand,
            (true, VirtualKey.B) or (false, VirtualKey.S) => ViewModel.SplitCommand,
            (false, VirtualKey.Delete) => ViewModel.DeleteSegmentCommand,
            (false, VirtualKey.Q) => ViewModel.TrimStartCommand,
            (false, VirtualKey.W) => ViewModel.TrimEndCommand,
            (false, VirtualKey.I) => ViewModel.SetMarkInCommand,
            (false, VirtualKey.O) => ViewModel.SetMarkOutCommand,
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
            Step((e.Key == VirtualKey.Left ? -1 : 1) * (shift ? 5 : 1));
        else if (e.Key == VirtualKey.Home)
            ViewModel.Seek(0);
        else if (e.Key == VirtualKey.End)
            ViewModel.Seek(ViewModel.Length);
        else if (e.Key is VirtualKey.Add or (VirtualKey)187)
            Timeline.ZoomBy(1.5);
        else if (e.Key is VirtualKey.Subtract or (VirtualKey)189)
            Timeline.ZoomBy(1 / 1.5);
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
