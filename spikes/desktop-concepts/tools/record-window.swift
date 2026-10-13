// Records one process's window, and nothing else on screen, to an .mp4.
//
//   record-window PID SECONDS OUT.mp4
//
// Stops after SECONDS, or when the window closes, whichever is first.
//
// Uses ScreenCaptureKit with a single-window filter, so other windows,
// menus and system overlays can never appear in the recording.
import AppKit
import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit

let args = CommandLine.arguments
guard args.count == 4, let pid = Int32(args[1]), let seconds = Double(args[2]) else {
    FileHandle.standardError.write("usage: record-window PID SECONDS OUT.mp4\n".data(using: .utf8)!)
    exit(2)
}
let out = URL(fileURLWithPath: args[3])
// Window capture needs a connection to the window server.
_ = NSApplication.shared
try? FileManager.default.removeItem(at: out)

final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate {
    let writer: AVAssetWriter
    let input: AVAssetWriterInput
    var started = false

    init(width: Int, height: Int) throws {
        writer = try AVAssetWriter(outputURL: out, fileType: .mp4)
        input = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: width,
            AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: 16_000_000],
        ])
        input.expectsMediaDataInRealTime = true
        writer.add(input)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, buffer.isValid,
              let info = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let raw = info.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete
        else { return }
        if !started {
            writer.startWriting()
            writer.startSession(atSourceTime: buffer.presentationTimeStamp)
            started = true
        }
        if input.isReadyForMoreMediaData { input.append(buffer) }
    }

    /// The window closed: keep what was recorded.
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        finish()
    }

    func finish() {
        guard started else { exit(1) }
        input.markAsFinished()
        writer.finishWriting {
            print(out.path)
            exit(0)
        }
    }
}

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
        guard let window = content.windows.first(where: { $0.owningApplication?.processID == pid && $0.windowLayer == 0 }) else {
            FileHandle.standardError.write("no window for pid \(pid)\n".data(using: .utf8)!)
            exit(1)
        }
        let config = SCStreamConfiguration()
        config.width = Int(window.frame.width * 2)
        config.height = Int(window.frame.height * 2)
        config.minimumFrameInterval = CMTime(value: 1, timescale: 60)
        config.showsCursor = false
        config.queueDepth = 6
        let recorder = try Recorder(width: config.width, height: config.height)
        let stream = SCStream(filter: SCContentFilter(desktopIndependentWindow: window), configuration: config, delegate: recorder)
        try stream.addStreamOutput(recorder, type: .screen, sampleHandlerQueue: DispatchQueue(label: "frames"))
        try await stream.startCapture()
        try await Task.sleep(nanoseconds: UInt64(seconds * 1e9))
        try? await stream.stopCapture()
        recorder.finish()
    } catch {
        FileHandle.standardError.write("record-window: \(error)\n".data(using: .utf8)!)
        exit(1)
    }
}
RunLoop.main.run()
