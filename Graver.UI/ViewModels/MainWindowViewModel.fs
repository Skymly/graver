namespace Graver.UI.ViewModels

open System
open System.Threading.Tasks
open System.Windows.Input
open Avalonia.Threading
open CommunityToolkit.Mvvm.Input
open Graver.UI

type MainWindowViewModel() as this =
    inherit ViewModelBase()

    let mutable status = "服务未连接"
    let mutable probing = false

    let pingCommand =
        RelayCommand(Action(fun () -> this.Probe()), Func<bool>(fun () -> not probing))

    member this.Status = status

    member this.PingCommand = pingCommand :> ICommand

    member this.Probe() =
        if not probing then
            probing <- true
            pingCommand.NotifyCanExecuteChanged()
            this.SetStatus("正在连接…")
            Task.Run(fun () ->
                let message = Ipc.probe ()
                Dispatcher.UIThread.Post(fun () ->
                    probing <- false
                    pingCommand.NotifyCanExecuteChanged()
                    this.SetStatus(message)))
            |> ignore

    member private this.SetStatus(value: string) =
        if status <> value then
            status <- value
            this.OnPropertyChanged("Status")
