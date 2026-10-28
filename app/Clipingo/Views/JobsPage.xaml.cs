using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Clipingo.Views;

public sealed partial class JobsPage : Page
{
    public JobsService Jobs { get; } = App.Services.GetRequiredService<JobsService>();

    public bool HasJobs => Jobs.Jobs.Count > 0;

    public JobsPage()
    {
        InitializeComponent();
    }

    /// <summary>Edit on a moment or an exported clip: the job is the data context of the card around it.</summary>
    private void EditClip_Click(object sender, RoutedEventArgs e)
    {
        var button = (FrameworkElement)sender;
        DependencyObject? at = button;
        while (at is not null && (at as FrameworkElement)?.DataContext is not JobItem)
            at = VisualTreeHelper.GetParent(at);
        if ((at as FrameworkElement)?.DataContext is JobItem { Run: { } run })
            App.MainWindow?.OpenEditor(run.Id, (int)button.Tag);
    }
}
