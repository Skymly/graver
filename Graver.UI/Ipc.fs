namespace Graver.UI

module Ipc =

    open System
    open System.Buffers.Binary
    open System.IO
    open System.IO.Pipes
    open System.Text

    let pipeName = "Graver"

    let private readExact (stream: Stream) (buffer: byte[]) =
        let mutable offset = 0
        while offset < buffer.Length do
            let n = stream.Read(buffer, offset, buffer.Length - offset)
            if n = 0 then
                failwith "连接在帧结束前关闭"
            offset <- offset + n

    let probe () =
        try
            use client = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut)
            client.Connect(800)
            client.ReadTimeout <- 1000
            client.WriteTimeout <- 1000
            let payload = Encoding.UTF8.GetBytes("""{"v":1,"id":1,"op":"ping"}""")
            let frame = Array.zeroCreate (4 + payload.Length)
            BinaryPrimitives.WriteUInt32LittleEndian(Span(frame, 0, 4), uint32 payload.Length)
            Buffer.BlockCopy(payload, 0, frame, 4, payload.Length)
            client.Write(frame, 0, frame.Length)
            client.Flush()

            let lengthBytes = Array.zeroCreate 4
            readExact client lengthBytes
            let length = int (BinaryPrimitives.ReadUInt32LittleEndian(ReadOnlySpan lengthBytes))
            if length <= 0 || length > 1024 * 1024 then
                "服务返回了无法接受的帧"
            else
                let body = Array.zeroCreate length
                readExact client body
                let text = Encoding.UTF8.GetString(body)
                if text.Contains("\"pong\"") then
                    "服务已响应"
                else
                    "服务返回了无法识别的内容"
        with
        | :? TimeoutException ->
            @"没有连上 \\.\pipe\Graver。请先运行 graver-service。"
        | :? IOException ->
            "管道已断开。请确认 graver-service 正在运行。"
        | ex ->
            "探测失败：" + ex.Message
