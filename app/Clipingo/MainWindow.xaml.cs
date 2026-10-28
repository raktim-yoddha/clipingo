using Clipingo.ViewModels;
using Clipingo.Views;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Clipingo;

public sealed partial class MainWindow : Window
{
    private static readonly Dictionary<string, Type> Pages = new()
    {
        ["Create"] = typeof(CreatePage),
        ["Library"] = typeof(LibraryPage),
        ["Editor"] = typeof(EditorPage),
        ["Jobs"] = typeof(JobsPage),
        ["Settings"] = typeof(SettingsPage),
    };

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
        AppWindow.Resize(new Windows.Graphics.SizeInt32(1360, 900));

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
        if (args.SelectedItem is NavigationViewItem { Tag: string tag } && Pages[tag] is var page && ContentFrame.CurrentSourcePageType != page)
            ContentFrame.Navigate(page, page == typeof(EditorPage) ? TakeEditorTarget() : null);
    }
}
