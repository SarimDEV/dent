import XCTest

/// Not a check: walks the main flows at a watchable pace for a recording.
/// Run on its own (`-only-testing:ConceptsUITests/DemoTests`).
final class DemoTests: XCTestCase {
    func testDemo() {
        let app = XCUIApplication()
        app.launchArguments = ["-look", "paper", "-pace", "1"]
        app.launch()
        let row = { (id: String) in app.descendants(matching: .any)["row-\(id)"].firstMatch }
        let pause = { (s: Double) in Thread.sleep(forTimeInterval: s) }
        pause(1.5)

        // Answer an approval; the card folds and the reply streams.
        row("s1").tap(); pause(1.5)
        app.buttons["allow"].tap(); pause(5)
        app.navigationBars.buttons.element(boundBy: 0).tap(); pause(2)

        // Close a finished session, undo, then close it for real.
        row("s5").swipeLeft(); pause(0.6)
        app.buttons["Close"].tap(); pause(1.5)
        app.buttons["undo"].tap(); pause(1.5)
        row("s5").swipeLeft(); pause(0.6)
        app.buttons["Close"].tap(); pause(2)

        // Closed sessions are archived, not gone: reopen one.
        let closed = app.descendants(matching: .any)["closed-list"].firstMatch
        for _ in 0..<3 where !closed.isHittable { app.swipeUp(); pause(0.4) }
        closed.tap(); pause(1.2)
        row("s5").tap(); pause(1.2)
        app.buttons["reopen"].tap(); pause(1)
        app.navigationBars.buttons.element(boundBy: 0).tap(); pause(0.8)
        app.navigationBars.buttons.element(boundBy: 0).tap(); pause(1.5)
        app.swipeDown(); pause(0.8)

        // Start a session by typing.
        app.buttons["new-session"].tap(); pause(1)
        app.typeText("Fix the usage line on the phone so it wraps"); pause(1.5)
        app.buttons["start"].tap(); pause(6)
        app.navigationBars.buttons.element(boundBy: 0).tap(); pause(2.5)
    }
}
