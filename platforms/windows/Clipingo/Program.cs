using Clipingo.Services;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace Clipingo;

/// <summary>The entry point (instead of the one XAML generates): the shared Windows App Runtime must start first.</summary>
public static class Program
{
    [STAThread]
    private static void Main()
    {
        if (!WindowsAppRuntime.Start())
            return;

        WinRT.ComWrappersSupport.InitializeComWrappers();
        Application.Start(_ =>
        {
            SynchronizationContext.SetSynchronizationContext(new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread()));
            new App();
        });
    }
}
