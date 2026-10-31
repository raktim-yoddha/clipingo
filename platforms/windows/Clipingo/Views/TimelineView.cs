using Clipingo.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Shapes;
using Windows.Foundation;

namespace Clipingo.Views;

/// <summary>
/// The editor's timeline: a time ruler, the clip's parts as a filmstrip (drag an edge to trim, drag a part to move
/// it, click to select and seek) and the captions under them (click a word to jump there). Ctrl + wheel zooms.
/// </summary>
public sealed partial class TimelineView : UserControl
{
    private const double RulerH = 22, TrackTop = 26, TrackH = 64, WordsTop = 96, WordsH = 26, Pad = 12, Edge = 10, ThumbH = 56;

    private readonly ScrollViewer _scroll = new()
    {
        HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
        VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
        HorizontalScrollMode = ScrollMode.Enabled,
        VerticalScrollMode = ScrollMode.Disabled,
        ZoomMode = ZoomMode.Disabled,
    };
    private readonly Canvas _canvas = new() { Background = new SolidColorBrush(Colors.Transparent), Height = WordsTop + WordsH + 6 };
    private readonly Canvas _parts = new();
    private readonly Rectangle _playhead = new() { Width = 2, Height = WordsTop + WordsH };
    private readonly Polygon _head = new() { Points = { new Point(-6, 0), new Point(6, 0), new Point(0, 8) } };
    private readonly Rectangle _drop = new() { Width = 3, Height = TrackH + 8, Visibility = Visibility.Collapsed, RadiusX = 1.5, RadiusY = 1.5 };
    private EditorViewModel? _vm;
    private int _redrawId;

    // Drag state
    private enum Drag { None, Scrub, TrimStart, TrimEnd, Move }
    private Drag _drag;
    private Segment? _dragSeg;
    private double _downX, _origStart, _origEnd;
    private bool _moved;
    private FrameworkElement? _dragVisual;

    /// <summary>Pixels per second (zoom).</summary>
    public double PxPerSecond { get; private set; } = 40;

    public TimelineView()
    {
        AutomationProperties.SetName(this, "Timeline");
        IsTabStop = true;
        _playhead.Fill = Res("AccentFillColorDefaultBrush");
        _head.Fill = Res("AccentFillColorDefaultBrush");
        _drop.Fill = Res("AccentFillColorDefaultBrush");
        Canvas.SetZIndex(_playhead, 10);
        Canvas.SetZIndex(_head, 10);
        Canvas.SetZIndex(_drop, 11);
        _canvas.Children.Add(_parts);
        _canvas.Children.Add(_playhead);
        _canvas.Children.Add(_head);
        _canvas.Children.Add(_drop);
        _canvas.PointerPressed += Canvas_PointerPressed;
        _canvas.PointerMoved += Canvas_PointerMoved;
        _canvas.PointerReleased += Canvas_PointerReleased;
        _canvas.PointerCaptureLost += (_, _) => EndDrag(commit: true);
        _canvas.PointerWheelChanged += Canvas_PointerWheelChanged;
        _scroll.Content = _canvas;
        Content = _scroll;
        SizeChanged += (_, _) => Redraw();
    }

    private static Brush Res(string key) => (Brush)Application.Current.Resources[key];

    public EditorViewModel? Model
    {
        get => _vm;
        set
        {
            if (_vm is not null)
            {
                _vm.TimelineChanged -= Redraw;
                _vm.PropertyChanged -= Vm_PropertyChanged;
            }
            _vm = value;
            if (_vm is not null)
            {
                _vm.TimelineChanged += Redraw;
                _vm.PropertyChanged += Vm_PropertyChanged;
            }
            Redraw();
        }
    }

    private void Vm_PropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(EditorViewModel.Position))
            MovePlayhead(follow: true);
        else if (e.PropertyName is nameof(EditorViewModel.IsFramingBusy))
            Redraw();
        else if (e.PropertyName == nameof(EditorViewModel.SelectedSegment))
            Redraw();
    }

    /// <summary>Zooms so the whole clip fits the visible width.</summary>
    public void Fit()
    {
        if (_vm is { Length: > 0 } && ActualWidth > 0)
            SetScale((ActualWidth - 2 * Pad - 8) / _vm.Length);
    }

    public void ZoomBy(double factor) => SetScale(PxPerSecond * factor);

    private void SetScale(double scale)
    {
        PxPerSecond = Math.Clamp(scale, 2, 400);
        Redraw();
    }

    private double X(double t) => Pad + t * PxPerSecond;

    private double T(double x) => (x - Pad) / PxPerSecond;

    private void MovePlayhead(bool follow = false)
    {
        if (_vm is null)
            return;
        var x = X(_vm.Position);
        Canvas.SetLeft(_playhead, x - 1);
        Canvas.SetLeft(_head, x);
        // Keep the playhead in view while playing.
        if (follow && _drag == Drag.None && (x < _scroll.HorizontalOffset || x > _scroll.HorizontalOffset + _scroll.ViewportWidth - 20))
            _scroll.ChangeView(Math.Max(0, x - 40), null, null, true);
    }

    /// <summary>Rebuilds the ruler, parts and words (cheap: a clip has a handful of parts).</summary>
    public void Redraw()
    {
        _parts.Children.Clear();
        if (_vm is null)
            return;
        var length = _vm.Length;
        _canvas.Width = Math.Max(ActualWidth - 4, X(length) + Pad + 40);
        DrawRuler(length);
        var id = ++_redrawId;
        double at = 0;
        foreach (var seg in _vm.Segments)
        {
            DrawPart(seg, at, id);
            at += seg.Length;
        }
        DrawWords();
        DrawShots();
        MovePlayhead();
    }

    private void DrawRuler(double length)
    {
        // A label every 1/2/5/10/15/30/60 s, whichever keeps them ~80 px apart.
        var step = new[] { 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600 }.FirstOrDefault(s => s * PxPerSecond >= 80, 900);
        var ink = Res("TextFillColorSecondaryBrush");
        var tick = Res("ControlStrongStrokeColorDefaultBrush");
        var end = Math.Max(length, T(_canvas.Width));
        for (double t = 0; t <= end; t += step)
        {
            var x = X(t);
            _parts.Children.Add(Place(new Rectangle { Width = 1, Height = 6, Fill = tick }, x, RulerH - 6));
            for (var k = 1; k < 4; k++)
                _parts.Children.Add(Place(new Rectangle { Width = 1, Height = 3, Fill = tick, Opacity = 0.6 }, x + k * step * PxPerSecond / 4, RulerH - 3));
            _parts.Children.Add(Place(new TextBlock { Text = EditorViewModel.Clock(t) + (step < 1 && t % 1 != 0 ? ".5" : ""), FontSize = 11, Foreground = ink }, x + 3, 1));
        }
    }

    private void DrawPart(Segment seg, double at, int id)
    {
        var w = Math.Max(4, seg.Length * PxPerSecond);
        var selected = ReferenceEquals(seg, _vm!.SelectedSegment);
        var strip = new StackPanel { Orientation = Orientation.Horizontal };
        var box = new Grid
        {
            Width = w,
            Height = TrackH,
            CornerRadius = new CornerRadius(6),
            Background = Res("ControlFillColorTertiaryBrush"),
            BorderBrush = selected ? Res("AccentFillColorDefaultBrush") : Res("ControlStrokeColorDefaultBrush"),
            BorderThickness = new Thickness(selected ? 2 : 1),
            Tag = seg,
        };
        AutomationProperties.SetName(box, seg.Label);
        box.Children.Add(new Border { Child = strip, Padding = new Thickness(0, (TrackH - ThumbH) / 2 - 1, 0, 0) });
        // Number and length, on a dark chip so they read over any frame.
        box.Children.Add(new Border
        {
            Background = new SolidColorBrush(ColorHelper.FromArgb(0xB0, 0, 0, 0)),
            CornerRadius = new CornerRadius(3),
            Padding = new Thickness(5, 1, 5, 1),
            Margin = new Thickness(6, 4, 0, 0),
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top,
            Visibility = w > 56 ? Visibility.Visible : Visibility.Collapsed,
            Child = new TextBlock { Text = $"{seg.Number}  {seg.Length:0.0}s", FontSize = 11, Foreground = new SolidColorBrush(Colors.White) },
        });
        // Trim handles.
        foreach (var left in new[] { true, false })
            box.Children.Add(new Border
            {
                Width = 6,
                Margin = new Thickness(2, 10, 2, 10),
                CornerRadius = new CornerRadius(3),
                HorizontalAlignment = left ? HorizontalAlignment.Left : HorizontalAlignment.Right,
                Background = selected ? Res("AccentFillColorDefaultBrush") : new SolidColorBrush(ColorHelper.FromArgb(0x90, 0xFF, 0xFF, 0xFF)),
                Visibility = w > 30 ? Visibility.Visible : Visibility.Collapsed,
            });
        box.Clip = new RectangleGeometry { Rect = new Rect(0, 0, w, TrackH) };
        _parts.Children.Add(Place(box, X(at), TrackTop));
        _ = FillStripAsync(seg, strip, w, id);
    }

    /// <summary>Frames along the part, one per thumbnail width (times rounded so zooming reuses them).</summary>
    private async Task FillStripAsync(Segment seg, StackPanel strip, double width, int id)
    {
        if (_drag is Drag.TrimStart or Drag.TrimEnd)
            return; // filled when the drag ends
        const double thumbW = ThumbH * 16 / 9;
        var count = Math.Max(1, (int)Math.Ceiling(width / thumbW));
        var step = seg.Length / count;
        var times = Enumerable.Range(0, count).Select(k => Math.Round((seg.Start + (k + 0.5) * step) * 2) / 2).ToArray();
        var paths = await _vm!.ThumbsAsync(times, (int)ThumbH);
        if (id != _redrawId)
            return;
        foreach (var p in paths)
        {
            strip.Children.Add(p.Length > 0
                ? new Image { Source = new BitmapImage(new Uri(p)), Width = thumbW, Height = ThumbH, Stretch = Stretch.UniformToFill, Opacity = 0.85 }
                : new Rectangle { Width = thumbW, Height = ThumbH });
        }
    }

    /// <summary>9:16: the shots along the bottom of the filmstrip, coloured by layout, and a dot on each zoom.</summary>
    private void DrawShots()
    {
        foreach (var s in _vm!.Framing)
        {
            var color = s.Layout switch
            {
                "stack" => ColorHelper.FromArgb(0xFF, 0x4A, 0xDE, 0x80),
                "split" => ColorHelper.FromArgb(0xFF, 0xFB, 0x92, 0x3C),
                "group" => ColorHelper.FromArgb(0xFF, 0xC0, 0x84, 0xFC),
                "whole" => ColorHelper.FromArgb(0xFF, 0x9C, 0xA3, 0xAF),
                _ => ColorHelper.FromArgb(0xFF, 0x60, 0xC5, 0xFA),
            };
            var bar = new Border { Width = Math.Max(2, (s.End - s.Start) * PxPerSecond - 2), Height = 4, CornerRadius = new CornerRadius(2), Background = new SolidColorBrush(color) };
            ToolTipService.SetToolTip(bar, s.Why);
            _parts.Children.Add(Place(bar, X(s.Start) + 1, TrackTop + TrackH - 6));
            foreach (var z in s.Zooms)
            {
                var dot = new Ellipse { Width = 8, Height = 8, Fill = new SolidColorBrush(Colors.White), Stroke = new SolidColorBrush(Colors.Black), StrokeThickness = 1 };
                ToolTipService.SetToolTip(dot, $"Zoom on \u201c{z.Word}\u201d");
                _parts.Children.Add(Place(dot, X(z.T) - 4, TrackTop + TrackH - 15));
            }
        }
    }

    private void DrawWords()
    {
        var fill = Res("ControlFillColorSecondaryBrush");
        var ink = Res("TextFillColorPrimaryBrush");
        foreach (var word in _vm!.TimelineWords)
        {
            var w = Math.Max(3, (word.Until - word.At) * PxPerSecond - 2);
            var chip = new Border
            {
                Width = w,
                Height = WordsH - 4,
                CornerRadius = new CornerRadius(3),
                Background = fill,
                Padding = new Thickness(3, 0, 3, 0),
                Child = w > 18 ? new TextBlock { Text = word.Text, FontSize = 11, Foreground = ink, TextTrimming = TextTrimming.Clip, VerticalAlignment = VerticalAlignment.Center } : null,
            };
            ToolTipService.SetToolTip(chip, word.Text);
            _parts.Children.Add(Place(chip, X(word.At) + 1, WordsTop));
        }
    }

    private static UIElement Place(FrameworkElement e, double x, double y)
    {
        Canvas.SetLeft(e, x);
        Canvas.SetTop(e, y);
        return e;
    }

    // ── Pointer ──

    private (Segment Seg, double Left)? PartAt(double x)
    {
        double at = 0;
        foreach (var seg in _vm!.Segments)
        {
            if (x >= X(at) && x <= X(at + seg.Length))
                return (seg, X(at));
            at += seg.Length;
        }
        return null;
    }

    private void Canvas_PointerPressed(object sender, PointerRoutedEventArgs e)
    {
        if (_vm is null || !_vm.HasClip)
            return;
        Focus(FocusState.Pointer);
        var p = e.GetCurrentPoint(_canvas).Position;
        _downX = p.X;
        _moved = false;
        _drag = Drag.Scrub;
        if (p.Y >= TrackTop && p.Y <= TrackTop + TrackH && PartAt(p.X) is var (seg, left))
        {
            var right = left + seg.Length * PxPerSecond;
            _dragSeg = seg;
            (_origStart, _origEnd) = (seg.Start, seg.End);
            _drag = p.X - left <= Edge ? Drag.TrimStart : right - p.X <= Edge ? Drag.TrimEnd : Drag.Move;
            _vm.SelectedSegment = seg;
            if (_drag != Drag.Move)
                _vm.Checkpoint();
        }
        if (_drag is Drag.Scrub or Drag.Move)
            _vm.Seek(T(p.X));
        _canvas.CapturePointer(e.Pointer);
        e.Handled = true;
    }

    private void Canvas_PointerMoved(object sender, PointerRoutedEventArgs e)
    {
        var p = e.GetCurrentPoint(_canvas).Position;
        if (_vm is null)
            return;
        if (_drag == Drag.None)
        {
            // Resize cursor over a part's edge.
            var edge = p.Y >= TrackTop && p.Y <= TrackTop + TrackH && PartAt(p.X) is var (seg, left)
                && (p.X - left <= Edge || left + seg.Length * PxPerSecond - p.X <= Edge);
            ProtectedCursor = InputSystemCursor.Create(edge ? InputSystemCursorShape.SizeWestEast : InputSystemCursorShape.Arrow);
            return;
        }
        var dx = p.X - _downX;
        _moved |= Math.Abs(dx) > 4;
        switch (_drag)
        {
            case Drag.Scrub:
                _vm.Seek(T(p.X));
                break;
            case Drag.TrimStart:
                _vm.TrimTo(_dragSeg!, _origStart + dx / PxPerSecond, _origEnd);
                LayoutParts();
                break;
            case Drag.TrimEnd:
                _vm.TrimTo(_dragSeg!, _origStart, _origEnd + dx / PxPerSecond);
                LayoutParts();
                break;
            case Drag.Move when _moved && _vm.Segments.Count > 1:
                _dragVisual ??= _parts.Children.OfType<Grid>().FirstOrDefault(g => ReferenceEquals(g.Tag, _dragSeg));
                if (_dragVisual is not null)
                {
                    _dragVisual.Opacity = 0.6;
                    _dragVisual.RenderTransform = new TranslateTransform { X = dx, Y = -6 };
                }
                var target = DropIndex(p.X);
                _drop.Visibility = Visibility.Visible;
                Canvas.SetLeft(_drop, X(_vm.StartOf(target)) - 1.5);
                Canvas.SetTop(_drop, TrackTop - 4);
                break;
        }
        e.Handled = true;
    }

    /// <summary>
    /// While an edge is dragged: move and resize the existing parts (a full redraw on every pointer move swaps the
    /// elements under the pointer and stalls the drag). Words hide until the drag ends.
    /// </summary>
    private void LayoutParts()
    {
        double at = 0;
        foreach (var seg in _vm!.Segments)
        {
            if (_parts.Children.OfType<Grid>().FirstOrDefault(g => ReferenceEquals(g.Tag, seg)) is { } box)
            {
                var w = Math.Max(4, seg.Length * PxPerSecond);
                box.Width = w;
                box.Clip = new RectangleGeometry { Rect = new Rect(0, 0, w, TrackH) };
                Canvas.SetLeft(box, X(at));
            }
            at += seg.Length;
        }
        foreach (var chip in _parts.Children.OfType<Border>())
            chip.Visibility = Visibility.Collapsed; // words and shot bars
    }

    /// <summary>The gap nearest the pointer: 0 = before the first part, Count = after the last.</summary>
    private int DropIndex(double x)
    {
        var t = T(x);
        double at = 0;
        var i = 0;
        foreach (var seg in _vm!.Segments)
        {
            if (t < at + seg.Length / 2)
                return i;
            at += seg.Length;
            i++;
        }
        return i;
    }

    private void Canvas_PointerReleased(object sender, PointerRoutedEventArgs e)
    {
        var x = e.GetCurrentPoint(_canvas).Position.X;
        if (_drag == Drag.Move && _moved && _dragSeg is not null && _vm is not null)
        {
            var target = DropIndex(x);
            var from = _vm.Segments.IndexOf(_dragSeg);
            _dragVisual = null;
            _drop.Visibility = Visibility.Collapsed;
            _drag = Drag.None;
            _vm.Move(_dragSeg, target > from ? target - 1 : target);
        }
        _canvas.ReleasePointerCapture(e.Pointer);
        EndDrag(commit: true);
        e.Handled = true;
    }

    private void EndDrag(bool commit)
    {
        var was = _drag;
        _drag = Drag.None;
        _drop.Visibility = Visibility.Collapsed;
        if (_dragVisual is not null)
        {
            _dragVisual.Opacity = 1;
            _dragVisual.RenderTransform = null;
            _dragVisual = null;
        }
        if (commit && was is Drag.TrimStart or Drag.TrimEnd && _vm is not null)
        {
            if (_dragSeg is { } s && (s.Start, s.End) == (_origStart, _origEnd))
            {
                _vm.DropCheckpoint();
                Redraw();
            }
            else
                _vm.Changed();
        }
        _dragSeg = null;
    }

    private void Canvas_PointerWheelChanged(object sender, PointerRoutedEventArgs e)
    {
        var ctrl = InputKeyboardSource.GetKeyStateForCurrentThread(Windows.System.VirtualKey.Control).HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);
        var delta = e.GetCurrentPoint(_canvas).Properties.MouseWheelDelta;
        if (ctrl)
        {
            ZoomBy(delta > 0 ? 1.25 : 0.8);
            e.Handled = true;
        }
        else if (!e.GetCurrentPoint(_canvas).Properties.IsHorizontalMouseWheel)
        {
            _scroll.ChangeView(_scroll.HorizontalOffset - delta, null, null, true);
            e.Handled = true;
        }
    }
}
