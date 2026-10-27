using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Storage.Pickers;
using Windows.System;

namespace Clipingo.Views;

public sealed partial class CreatePage : Page
{
    public CreateViewModel ViewModel { get; } = App.Services.GetRequiredService<CreateViewModel>();

    public CreatePage()
    {
        InitializeComponent();
    }

    private async void Browse_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            var picker = new FileOpenPicker { SuggestedStartLocation = PickerLocationId.VideosLibrary };
            foreach (var ext in new[] { ".mp4", ".mkv", ".mov", ".webm", ".flv", ".ts" })
                picker.FileTypeFilter.Add(ext);
            WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(App.MainWindow));
            if (await picker.PickSingleFileAsync() is { } file)
            {
                ViewModel.SourceInput = file.Path;
                await ViewModel.ProbeCommand.ExecuteAsync(null);
            }
        }
        catch (Exception ex)
        {
            ViewModel.ErrorMessage = ex.Message;
        }
    }

    private void SourceBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter && ViewModel.ProbeCommand.CanExecute(null))
            ViewModel.ProbeCommand.Execute(null);
    }
}
