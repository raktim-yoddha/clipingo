using Clipingo.Services;
using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml;

namespace Clipingo;

public partial class App : Application
{
    /// <summary>Composition root; pages resolve their view models here.</summary>
    public static IServiceProvider Services { get; } = new ServiceCollection()
        .AddSingleton<CoreClient>()
        .AddSingleton<Func<CoreClient>>(() => new CoreClient())
        .AddSingleton<JobsService>()
        .AddSingleton<CreateViewModel>()
        .AddSingleton<EditorViewModel>()
        .AddSingleton<UpdatesViewModel>()
        .AddTransient<LibraryViewModel>()
        .AddTransient<SettingsViewModel>()
        .BuildServiceProvider();

    public static MainWindow? MainWindow { get; private set; }

    public App()
    {
        InitializeComponent();
        // Unhandled XAML errors otherwise only show up as 0xc000027b in the event log.
        UnhandledException += (_, e) =>
        {
            try
            {
                var dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Clipingo", "logs");
                Directory.CreateDirectory(dir);
                File.AppendAllText(Path.Combine(dir, "app.log"), $"{DateTime.Now:O} {e.Exception}\n\n");
            }
            catch (IOException)
            {
            }
        };
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        MainWindow = new MainWindow();
        MainWindow.Activate();
    }
}
