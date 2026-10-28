using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml.Controls;

namespace Clipingo.Views;

public sealed partial class JobsPage : Page
{
    public JobsService Jobs { get; } = App.Services.GetRequiredService<JobsService>();

    public bool HasJobs => Jobs.Jobs.Count > 0;

    public JobsPage()
    {
        InitializeComponent();
    }
}
