# Phone design concepts

A working prototype of the iPhone app on a fake store (`Concepts/Store.swift`): sessions, approvals, streaming replies, queueing, stop and resume, closing as archive with undo. Every iPhone screen with fake data, built only from stock SwiftUI on iOS 26, in four looks (system, paper, serif, book). Each screen is a real navigation state, so back buttons, sheets and the keyboard are the system's own. The Lock Screen, notification actions, Dynamic Island and widgets are drawings of those surfaces.

```sh
cd spikes/phone-concepts && xcodegen generate
xcodebuild build -project Concepts.xcodeproj -scheme Concepts \
  -destination 'platform=iOS Simulator,name=iPhone 17' -derivedDataPath build/dd
node shoot.mjs OUT --looks paper [--appearance dark] [--text-size accessibility-extra-extra-extra-large]
```

With no `-screen` argument the app is interactive. The UI tests drive every flow and check the edges (a double tap on Send, Stop mid-reply, sending while a reply streams, the keyboard over the composer):

```sh
xcodebuild build-for-testing … && TEST_RUNNER_SHOTS=/tmp/flows xcodebuild test-without-building -project Concepts.xcodeproj \
  -scheme Concepts -destination 'platform=iOS Simulator,name=iPhone 17' -derivedDataPath build/dd -only-testing:ConceptsUITests/FlowTests
```

`DemoTests` walks the flows at a watchable pace for recordings.

Screens are reviewed against [docs/design/iphone-checklist.md](../../docs/design/iphone-checklist.md) by the `design-review` agent in `.claude/agents/`.
