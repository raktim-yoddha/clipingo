using Clipingo.ViewModels;
using Clipingo.Views;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Clipingo;

public sealed partial class MainWindow : Window
{
    /// <summary>Widest a page but the editor grows: past this, lines get too long to read.</summary>
    private const double PageMaxWidth = 1240;

    private static readonly Dictionary<string, Type> Pages = new()
    {
        ["Create"] = typeof(CreatePage),
        ["Library"] = typeof(LibraryPage),
        ["Editor"] = typeof(EditorPage),
        ["Jobs"] = typeof(JobsPage),
        ["Settings"] = typeof(SettingsPage),
    };

    [System.Runtime.InteropServices.DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hwnd);

    public UpdatesViewModel Updates { get; } = App.Services.GetRequiredService<UpdatesViewModel>();

    public MainWindow()
    {
        InitializeComponent();
        // The installer replaces the app's files: close so it can, it starts Clipingo again afterwards.
        Updates.ReadyToQuit += () => DispatcherQueue.TryEnqueue(Close);
        _ = Updates.StartupCheckAsync();

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredTheme = TitleBarTheme.UseDefaultAppMode;
        AppWindow.SetIcon("Assets/AppIcon.ico");
        // 1360×900 (scaled to the display), but never past the screen: the editor's timeline sits at the bottom.
        var area = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        var dpi = GetDpiForWindow(Microsoft.UI.Win32Interop.GetWindowFromWindowId(AppWindow.Id));
        var scale = dpi > 0 ? dpi / 96.0 : 1;
        var (w, h) = (Math.Min((int)(1360 * scale), area.Width - 32), Math.Min((int)(900 * scale), area.Height - 32));
        AppWindow.MoveAndResize(new Windows.Graphics.RectInt32(area.X + (area.Width - w) / 2, area.Y + (area.Height - h) / 2, w, h));

        Nav.SelectedItem = CreateItem;
    }

    /// <summary>Selects a sidebar page by its tag ("Create", "Library", "Jobs", "Settings").</summary>
    public void Navigate(string tag) =>
        Nav.SelectedItem = Nav.MenuItems.Concat(Nav.FooterMenuItems).OfType<NavigationViewItem>().First(i => (string)i.Tag == tag);

    /// <summary>Opens the editor on a run (and one of its clips).</summary>
    public void OpenEditor(string runId, int? rank)
    {
        _editorTarget = new EditorTarget(runId, rank);
        if (ReferenceEquals(Nav.SelectedItem, EditorItem))
            ContentFrame.Navigate(typeof(EditorPage), TakeEditorTarget());
        else
            Nav.SelectedItem = EditorItem;
    }

    private EditorTarget? _editorTarget;

    private EditorTarget? TakeEditorTarget()
    {
        var t = _editorTarget;
        _editorTarget = null;
        return t;
    }

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is not NavigationViewItem { Tag: string tag })
            return;
        // The editor needs the width: icons only there.
        Nav.PaneDisplayMode = tag == "Editor" ? NavigationViewPaneDisplayMode.LeftCompact : NavigationViewPaneDisplayMode.Left;
        PageColumn.MaxWidth = tag == "Editor" ? double.PositiveInfinity : PageMaxWidth;
        if (Pages[tag] is var page && ContentFrame.CurrentSourcePageType != page)
            ContentFrame.Navigate(page, page == typeof(EditorPage) ? TakeEditorTarget() : null);
    }
}
