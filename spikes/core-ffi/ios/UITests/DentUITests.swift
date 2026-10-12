import XCTest

/// The phone flow end to end against a real daemon on this Mac: find it
/// with Bonjour, pair with its code, send a prompt, watch the reply. The
/// harness passes the code in DENT_CODE (TEST_RUNNER_DENT_CODE) and a folder
/// for screenshots in DENT_SHOTS.
final class DentUITests: XCTestCase {
    let env = ProcessInfo.processInfo.environment

    func shot(_ name: String) {
        guard let dir = env["DENT_SHOTS"] else { return }
        let png = XCUIScreen.main.screenshot().pngRepresentation
        try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name).png"))
    }

    /// Tell the harness where we are (it restarts the daemon on cue).
    func mark(_ name: String) {
        guard let dir = env["DENT_SHOTS"] else { return }
        try? Data().write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name).mark"))
    }

    func testPairAndChat() throws {
        let app = XCUIApplication()
        app.launchArguments = ["-fresh"]
        if let u = env["DENT_PUSHURL"] { app.launchArguments += ["-pushurl", u, "-pushtoken", "sim-uitest"] }
        if let a = env["DENT_ADDR"] { app.launchArguments += ["-addr", a] }
        app.launch()

        let add = app.buttons["add-studio"]
        XCTAssertTrue(add.waitForExistence(timeout: 20), "studio found nearby")
        sleep(1)
        shot("1-found")
        mark("found")
        add.tap()

        let code = app.textFields["code"]
        XCTAssertTrue(code.waitForExistence(timeout: 10))
        sleep(1)
        shot("2-code")
        code.typeText(env["DENT_CODE"] ?? "000000")
        sleep(2)

        // Notifications are asked for once the first machine is paired.
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow"]
        if allow.waitForExistence(timeout: 4) { allow.tap() }

        let composer = app.textViews["composer"].exists ? app.textViews["composer"] : app.textFields["composer"]
        if !composer.waitForExistence(timeout: 20) {
            shot("fail-pairing")
            XCTFail("chat didn't open once paired; on screen: \(app.staticTexts.allElementsBoundByIndex.map(\.label))")
            return
        }
        composer.tap()
        composer.typeText("Why does the sync test fail after a reconnect?")
        app.buttons["send"].tap()
        mark("sent")
        sleep(2)
        shot("3-streaming")
        XCTAssertTrue(app.staticTexts["Done"].waitForExistence(timeout: 30), "reply finishes")
        sleep(1)
        shot("4-reply")

        // Leave the app; the next reply's notification arrives on the home
        // screen, opened by the extension.
        if env["DENT_PUSHURL"] != nil {
            composer.tap()
            composer.typeText("And the second flaky test?")
            app.buttons["send"].tap()
            XCUIDevice.shared.press(.home)
            sleep(8)
            shot("5-notification")
        }
    }
}
