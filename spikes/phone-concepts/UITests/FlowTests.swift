import XCTest

/// Every flow in the prototype, driven like a person would, with screenshots
/// written to $SHOTS. Edge cases covered here or by `shoot.mjs` stills:
///
/// - Close, undo; close, reopen from Closed; Closed when empty
/// - Allow: the card folds, the reply streams, the row lands in Done
/// - New session: keyboard up on open, project guessed while typing,
///   defaults stay above the keyboard, start opens the chat, row in Working
/// - Cancel a new session with text, reopen: the draft is still there
/// - Chat: a five-line draft keeps the composer above the keyboard;
///   the sent message is visible, not hidden behind the composer
/// - Queue while working; stop with nothing typed
/// - A double tap on Send never stops the agent
/// - Stop really stops (no reply lands afterwards) and offers Resume
/// - Sending while a reply streams queues; nothing already arrived is lost
/// - Needs-you rows can't be closed; a closed chat offers Reopen
/// - Search finds open and closed sessions
/// - Stills: dark, the largest text size, 40 sessions with long titles,
///   iPhone 16e and 17 Pro Max, Reduce Motion
final class FlowTests: XCTestCase {
    var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchArguments = ["-look", "paper", "-pace", "1"]
        app.launch()
    }

    func shot(_ name: String) {
        guard let dir = ProcessInfo.processInfo.environment["SHOTS"] else { return }
        let png = XCUIScreen.main.screenshot().pngRepresentation
        try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name).png"))
    }

    func row(_ id: String) -> XCUIElement { app.descendants(matching: .any)[ "row-\(id)"].firstMatch }

    func back() { app.navigationBars.buttons.element(boundBy: 0).tap() }

    /// The keyboard's top edge, or the screen's bottom when it's hidden.
    var keyboardTop: CGFloat {
        let kb = app.keyboards.firstMatch
        return kb.exists ? kb.frame.minY : app.frame.maxY
    }

    func assertAboveKeyboard(_ e: XCUIElement, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertLessThanOrEqual(e.frame.maxY, keyboardTop + 1, "\(what) is under the keyboard", file: file, line: line)
    }

    // MARK: Closing

    func testCloseUndoAndReopen() {
        let done = row("s5")
        XCTAssertTrue(done.waitForExistence(timeout: 5))
        done.swipeLeft()
        app.buttons["Close"].tap()
        XCTAssertTrue(done.waitForNonExistence(timeout: 3), "closed row should leave the list")
        let undo = app.buttons["undo"]
        XCTAssertTrue(undo.waitForExistence(timeout: 2), "an undo should appear")
        XCTAssertEqual(undo.label, "Undo")
        shot("flow-close-toast")
        undo.tap()
        XCTAssertTrue(done.waitForExistence(timeout: 3), "undo brings it back")

        // Close again and reopen it from Closed.
        done.swipeLeft()
        app.buttons["Close"].tap()
        XCTAssertTrue(done.waitForNonExistence(timeout: 3))
        let closed = app.descendants(matching: .any)["closed-list"].firstMatch
        for _ in 0..<4 where !closed.isHittable { app.swipeUp() }
        closed.tap()
        let inClosed = row("s5")
        XCTAssertTrue(inClosed.waitForExistence(timeout: 3))
        shot("flow-closed-list")
        // A closed chat reads normally and offers Reopen in place of the composer.
        inClosed.tap()
        XCTAssertTrue(app.buttons["reopen"].waitForExistence(timeout: 3))
        shot("flow-closed-chat")
        back()
        inClosed.swipeRight()
        app.buttons["Reopen"].tap()
        back()
        XCTAssertTrue(row("s5").waitForExistence(timeout: 3), "reopened session is back in the list")
    }

    // MARK: Approving

    func testAllowFoldsAndFinishes() {
        row("s1").tap()
        let allow = app.buttons["allow"]
        XCTAssertTrue(allow.waitForExistence(timeout: 5))
        shot("flow-approval")
        allow.tap()
        XCTAssertTrue(app.descendants(matching: .any)["resolved"].firstMatch.waitForExistence(timeout: 3), "the card folds to a line")
        XCTAssertFalse(allow.exists)
        XCTAssertTrue(app.descendants(matching: .any)["streaming"].firstMatch.waitForExistence(timeout: 6), "the reply streams")
        shot("flow-streaming")
        let reply = app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'All 212 tests pass'")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 15), "the reply finishes")
        back()
        XCTAssertTrue(app.staticTexts["212 tests pass · ready for review"].waitForExistence(timeout: 3), "the row lands in Done")
        shot("flow-after-allow")
    }

    // MARK: New session

    func testNewSessionGuessesProjectAndStarts() {
        app.buttons["new-session"].tap()
        let field = app.textViews["new-composer"].exists ? app.textViews["new-composer"] : app.textFields["new-composer"]
        XCTAssertTrue(field.waitForExistence(timeout: 3))
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3), "keyboard is up straight away")
        let chip = app.buttons["chip-project"]
        XCTAssertTrue(chip.label.contains("Project from message"))
        field.typeText("Fix the usage line on the phone so it wraps")
        let guessed = expectation(for: NSPredicate(format: "label CONTAINS 'dent'"), evaluatedWith: chip)
        wait(for: [guessed], timeout: 2)   // the guess settles after a short pause in typing
        assertAboveKeyboard(chip, "the project menu")
        assertAboveKeyboard(app.buttons["start"], "the start button")
        shot("flow-new-typed")
        app.buttons["start"].tap()
        sleep(1)
        shot("flow-new-after-start")
        XCTAssertTrue(app.staticTexts["Fix the usage line on the phone so it wraps"].waitForExistence(timeout: 3), "chat opens with the message")
        let finished = app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'subtitle for the count'")).firstMatch
        XCTAssertTrue(finished.waitForExistence(timeout: 10), "the agent's reply arrives")
        shot("flow-new-started")
        back()
        XCTAssertTrue(app.staticTexts["Fix the usage line on the…"].waitForExistence(timeout: 3), "the new session is in the list")
    }

    func testCancelledDraftIsKept() {
        app.buttons["new-session"].tap()
        let field = app.textViews["new-composer"].exists ? app.textViews["new-composer"] : app.textFields["new-composer"]
        XCTAssertTrue(field.waitForExistence(timeout: 3))
        field.typeText("Half a thought")
        app.buttons["Cancel"].tap()
        XCTAssertTrue(app.buttons["new-session"].waitForExistence(timeout: 3))
        app.buttons["new-session"].tap()
        XCTAssertTrue(app.descendants(matching: .any).matching(NSPredicate(format: "value == 'Half a thought'")).firstMatch.waitForExistence(timeout: 3),
                      "the draft survives cancelling")
    }

    // MARK: Keyboard in a chat

    func testComposerStaysAboveKeyboard() {
        row("s5").tap()
        let composer = app.textViews["composer"].exists ? app.textViews["composer"] : app.textFields["composer"]
        XCTAssertTrue(composer.waitForExistence(timeout: 3))
        composer.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        composer.typeText("One\nTwo\nThree\nFour\nFive lines of draft")
        assertAboveKeyboard(composer, "the composer")
        assertAboveKeyboard(app.buttons["send"], "the send button")
        shot("flow-keyboard-long-draft")
        app.buttons["send"].tap()
        // While it works, the button is Stop; typing turns it into Send (queue).
        let button = app.buttons["send"]
        XCTAssertTrue(button.waitForExistence(timeout: 1))
        XCTAssertEqual(button.label, "Stop")
        let sent = app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'Five lines of draft'")).firstMatch
        XCTAssertTrue(sent.waitForExistence(timeout: 3))
        // Sent message sits above the composer, not behind it.
        XCTAssertLessThanOrEqual(sent.frame.maxY, composer.frame.minY + 1, "sent message is hidden behind the composer")
        shot("flow-keyboard-sent")
        composer.typeText("and one more")
        XCTAssertEqual(app.buttons["send"].label, "Send", "typing while working queues")
    }

    // MARK: Search

    func testSearchFindsSessions() {
        let search = app.buttons["Search"].firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 3))
        search.tap()
        app.typeText("pricing")
        XCTAssertTrue(row("s2").waitForExistence(timeout: 3))
        shot("flow-search")
    }

    // MARK: The edges the reviewer found

    func testNeedsYouCannotBeClosed() {
        row("s1").swipeLeft()
        XCTAssertFalse(app.buttons["Close"].waitForExistence(timeout: 1), "a session waiting on you must be answered, not closed")
    }

    func openComposer(_ id: String) -> XCUIElement {
        row(id).tap()
        let c = app.textViews["composer"].exists ? app.textViews["composer"] : app.textFields["composer"]
        XCTAssertTrue(c.waitForExistence(timeout: 3))
        c.tap()
        return c
    }

    func testDoubleTapOnSendDoesNotStop() {
        let c = openComposer("s5")
        c.typeText("Also update the docs")
        let button = app.buttons["send"]
        button.doubleTap()   // a real double tap: two taps about 0.1 s apart
        sleep(1)
        XCTAssertFalse(app.staticTexts["Stopped by you"].exists, "a double tap on Send stopped the agent")
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH 'On it.'")).firstMatch.waitForExistence(timeout: 8))
    }

    func testStopStopsAndResumes() {
        let c = openComposer("s5")
        c.typeText("Rename the flag")
        app.buttons["send"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["streaming"].firstMatch.waitForExistence(timeout: 6))
        let button = app.buttons["send"]
        XCTAssertEqual(button.label, "Stop")
        button.tap()
        XCTAssertTrue(app.staticTexts["Stopped by you"].waitForExistence(timeout: 2))
        XCTAssertTrue(app.buttons["undo"].waitForExistence(timeout: 2), "Stop offers Resume")
        XCTAssertEqual(app.buttons["undo"].label, "Resume")
        shot("flow-stopped")
        sleep(2)   // well inside the 8 s Resume window
        XCTAssertFalse(app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH 'On it.' AND label ENDSWITH 'any further.'")).firstMatch.exists, "the reply kept arriving after Stop")
        app.buttons["undo"].tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH 'Picking up where I stopped'")).firstMatch.waitForExistence(timeout: 10))
    }

    func testStopFromTheListOffersResume() {
        let working = row("s3")
        for _ in 0..<4 where !(working.exists && working.isHittable) { app.swipeUp() }
        working.swipeLeft()
        app.buttons["Stop"].tap()
        let resume = app.buttons["undo"]
        XCTAssertTrue(resume.waitForExistence(timeout: 2), "stopping from the list shows Resume there")
        XCTAssertEqual(resume.label, "Resume")
        // After the toast, Resume is still a swipe away on the row.
        sleep(9)
        XCTAssertFalse(resume.exists)
        // The stopped session moved up to Done; bring it into view.
        for _ in 0..<4 where !(working.exists && working.isHittable) { app.swipeDown() }
        shot("flow-stopped-row")
        working.swipeRight()
        XCTAssertTrue(app.buttons["Resume"].waitForExistence(timeout: 2))
        app.buttons["Resume"].tap()
        // It works again (Working), then finishes (Done); either shows the resume took.
        let again = app.staticTexts.matching(NSPredicate(format: "label IN {'Picking up where it stopped', 'Done · ready for review'}")).firstMatch
        XCTAssertTrue(again.waitForExistence(timeout: 8), "the session works again")
    }

    func testSendingWhileStreamingQueues() {
        // Slower agents, so the queued state is on screen long enough to capture.
        app.terminate()
        app.launchArguments = ["-look", "paper", "-pace", "2.5"]
        app.launch()
        let c = openComposer("s5")
        c.typeText("First thing")
        app.buttons["send"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["streaming"].firstMatch.waitForExistence(timeout: 6))
        c.typeText("Second thing")
        app.buttons["send"].tap()
        shot("flow-queued")
        // Outcome, in order: the first reply whole, then the queued message, then its reply.
        let first = app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH 'On it.' AND label ENDSWITH 'any further.'")).firstMatch
        let next = app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH 'Done with that'")).firstMatch
        XCTAssertTrue(next.waitForExistence(timeout: 30), "the queued message runs after the first reply")
        XCTAssertTrue(first.exists, "the first reply arrived whole")
        let second = app.staticTexts["Second thing"]
        XCTAssertTrue(second.exists)
        XCTAssertLessThan(first.frame.minY, second.frame.minY, "queued message sits after the reply it waited for")
        XCTAssertLessThan(second.frame.minY, next.frame.minY)
        XCTAssertFalse(app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS 'Queued'")).firstMatch.exists)
    }
}
