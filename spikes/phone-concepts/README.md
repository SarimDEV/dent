# Phone design concepts

Every iPhone screen with fake data, built only from stock SwiftUI on iOS 26, in four looks (system, paper, serif, book). Each screen is a real navigation state, so back buttons, sheets and the keyboard are the system's own. The Lock Screen, notification actions, Dynamic Island and widgets are drawings of those surfaces.

```sh
cd spikes/phone-concepts && xcodegen generate
xcodebuild build -project Concepts.xcodeproj -scheme Concepts \
  -destination 'platform=iOS Simulator,name=iPhone 17' -derivedDataPath build/dd
node shoot.mjs OUT --looks paper [--appearance dark] [--text-size accessibility-extra-extra-extra-large]
```

Screens are reviewed against [docs/design/iphone-checklist.md](../../docs/design/iphone-checklist.md) by the `design-review` agent in `.claude/agents/`.
