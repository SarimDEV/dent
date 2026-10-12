# iPhone design checklist

Use this checklist to review every screen of the dent iPhone app. The app is SwiftUI on iOS 26 with Liquid Glass. Each rule is checkable and comes from Apple's Human Interface Guidelines (HIG), the WWDC25 design sessions, or the SwiftUI reference. Each rule names the native SwiftUI API that implements it. The default answer is to use the system component. A custom view needs a reason we can write down, and if we can't name one, the custom view is a bug.

How to use it: for each screen, go through Structure, Type, Color, Touch and the component sections that apply. Then check the screen against "Common vibe-coded smells". Check it at the default text size and at AX5, in light mode and dark mode, and on the smallest and largest iPhone.

Tags: **[HIG]** Human Interface Guidelines page, **[S356]** "Get to know the new design system", **[S219]** "Meet Liquid Glass", **[S323]** "Build a SwiftUI app with the new design", **[API]** SwiftUI reference. All links are in Sources.

---

## 1. iOS 26: what changed from older iOS

- [ ] **Controls float, content doesn't.** Liquid Glass is "a distinct functional layer for controls and navigation elements" that floats above content [HIG Materials]. Bars no longer sit on an opaque strip. Content scrolls underneath them and a scroll edge effect keeps them legible [S356].
- [ ] **The tab bar floats** above content at the bottom. It can minimize on scroll and can carry an accessory such as a mini player [HIG Tab bars]. API: `TabView { Tab(...) }`, `.tabBarMinimizeBehavior(.onScrollDown)`, `.tabViewBottomAccessory { }` [S323, API].
- [ ] **Search moved to the bottom.** On iPhone, `.searchable` in a toolbar puts the field at the bottom. A multi-tab app can have a dedicated search tab with `Tab(role: .search)` [S323, HIG Search fields].
- [ ] **Toolbar items group automatically** on a shared glass surface. Use `ToolbarSpacer(.fixed)` to split the groups and `ToolbarSpacer(.flexible)` to push them apart [S323].
- [ ] **The primary action is tinted and separate.** Done and Submit use the prominent style on the trailing edge, often as a blue checkmark [HIG Toolbars, S356]. API: `Button(role: .confirm)` or `.buttonStyle(.glassProminent)`, placed with `ToolbarItem(placement: .confirmationAction)`.
- [ ] **Close and Back are symbols, not words.** "Don't use a text label that says Back or Close" [HIG Toolbars]. API: `Button(role: .close)` (iOS 26) and the system back button.
- [ ] **Partial-height sheets are inset glass by default.** Remove `.presentationBackground(...)` and "let the new material shine" [S323].
- [ ] **Menus, alerts, popovers and confirmation dialogs grow out of the control** that presented them [S323, S356]. Attach them to that control so the animation has a source.
- [ ] **Bordered buttons are capsules by default.** Shapes are fixed, capsule or concentric. On phone, use "a capsule with extra margin" near the screen edge [S356]. API: `.buttonBorderShape(.capsule)`, `.rect(corner: .containerConcentric)` [S323].
- [ ] **Type is bolder and left-aligned** in alerts and onboarding [S356]. Don't center long text by hand.
- [ ] **Remove custom bar styling.** Extra backgrounds and borders on bars are "unnecessary" and "will interfere with the effect" [S356, S323].
- [ ] **Accessibility settings change glass automatically.** Reduce Transparency makes it frostier, Increase Contrast makes it black or white with a border, and Reduce Motion removes elasticity [S219]. Custom glass gets this only through `glassEffect`, not a hand-made blur.

## 2. Liquid Glass rules

- [ ] **Glass is for the navigation and control layer only.** "Don't use Liquid Glass in the content layer" [HIG Materials]. Rows, cards, chat bubbles and backgrounds are content. Give them system background colors or standard materials (`.ultraThinMaterial` … `.thickMaterial`), never `glassEffect`. "A table view should stay out of glass" [S219]. The one exception: sliders and toggles take on glass briefly while someone is using them [HIG Materials].
- [ ] **Never put glass on glass.** "Always avoid glass on glass" [S219]. An element on top of glass uses fills, transparency and vibrancy, not a second layer of glass. Glass cannot sample other glass, so put neighbouring custom glass elements in one `GlassEffectContainer` [S323].
- [ ] **Use standard components first.** They pick up glass automatically. Add custom glass "sparingly", and only to the most important functional elements [HIG Materials]. API: `.glassEffect(_:in:)`, `.buttonStyle(.glass)`, `.glassEffect(.regular.interactive())` for custom touch controls [S323].
- [ ] **Apply glass to the control itself, not its inner views** [S356].
- [ ] **Use the regular variant by default.** Clear glass needs all three: media-rich content behind it, a dimming layer that doesn't hurt that content, and bold, bright content on top [S219]. Over bright content, add a dark dimming layer at 35% opacity [HIG Materials]. Never mix regular and clear [S219].
- [ ] **Tint only primary actions.** "When every element is tinted, nothing stands out" [S219]. To emphasize an action, put the color on the background, not on the symbol or text, and never tint the backgrounds of several controls [HIG Color]. API: `.glassEffect(.regular.tint(.accentColor))`, `.buttonStyle(.glassProminent)`.
- [ ] **Bar icons are monochrome.** Use `.tint` on a toolbar item only for meaning, such as a call to action [S323].
- [ ] **No opaque backgrounds under bars.** Do not use `.toolbarBackground(Color…, for: .navigationBar)` or a colored `Rectangle` behind the tab bar. Let the scroll edge effect separate bar from content [HIG Layout, HIG Toolbars]. Use `.scrollEdgeEffectStyle(.hard, for: .top)` only for dense UIs with pinned headers [S323, S219].
- [ ] **One scroll edge effect per view.** Don't mix or stack the styles, and don't use it where nothing floats [S356].
- [ ] **In resting states, content doesn't overlap glass**, for example the top of a list at launch. Reposition or scale the content instead [S219, HIG Color].
- [ ] **Extend full-bleed backgrounds under the bars.** Mirror art under bars with `.backgroundExtensionEffect()` [HIG Layout, S323].

## 3. Structure and navigation

- [ ] **Each tab root is a `NavigationStack` with `.navigationTitle("…")`.** The root screen shows a large title that collapses to inline on scroll [HIG Toolbars, iOS]. API: the large title is automatic at the root. Force it with `.toolbarTitleDisplayMode(.large)` and use `.inline` on detail screens.
- [ ] **Titles name the screen, not the app.** "Don't title windows with your app name". Keep titles under 15 characters [HIG Toolbars]. So the title is "Sessions", not "dent" or a wordmark.
- [ ] **Bar items use the three toolbar areas: leading, center, trailing.** Use at most three groups. Don't put a text button next to a symbol button without a spacer, because the two read as one [HIG Toolbars, S356]. API: `.toolbar { ToolbarItem(placement: .topBarTrailing) … }`, `ToolbarSpacer`.
- [ ] **Overflow goes into a More menu**, not a crowded bar. API: `Menu { … } label: { Label("More", systemImage: "ellipsis") }` [HIG Toolbars, S356]. A `ControlGroup` holds related actions that belong together [API].
- [ ] **Bar items are SF Symbols without borders.** Don't use outlined-circle symbols, because the bar already provides a container [HIG Toolbars]. Use text when no symbol is a clear shorthand [S356].
- [ ] **Tab bars are for navigation, never actions.** Put screen-specific actions with the content [HIG Tab bars, S356].
- [ ] **Tab labels are single words with filled SF Symbols.** Never hide or disable a tab. If a tab is empty, explain why. Avoid the "More" overflow tab [HIG Tab bars]. API: `Tab("Inbox", systemImage: "tray.fill") { … }`, and `.badge(n)` only for critical information.
- [ ] **The tab bar stays visible** while people drill in. Only modals cover it [HIG Tab bars].
- [ ] **Swipe back works everywhere.** Use the system back button, no custom back chevron, and never block the edge swipe [HIG Toolbars, HIG Designing for iOS].
- [ ] **Layout follows size classes, not device or orientation** [HIG Layout]. API: `@Environment(\.horizontalSizeClass)`, `ViewThatFits`.
- [ ] **Respect safe areas.** Nothing interactive sits under the Dynamic Island or home indicator. Remove `.ignoresSafeArea()` from content (backgrounds are fine) [HIG Layout].
- [ ] **Relaunch restores state**: the same tab, the same session, the same scroll position [HIG Launching]. API: `@SceneStorage`.

## 4. Typography and Dynamic Type

- [ ] **Every text uses a text style, never `.font(.system(size:))`.** iOS sizes at Large, the default [HIG Typography]:

  | Style | SwiftUI | Weight | Size / leading (pt) | AX5 size |
  | --- | --- | --- | --- | --- |
  | Large Title | `.largeTitle` | Regular | 34 / 41 | 60 |
  | Title 1 | `.title` | Regular | 28 / 34 | 58 |
  | Title 2 | `.title2` | Regular | 22 / 28 | 56 |
  | Title 3 | `.title3` | Regular | 20 / 25 | 55 |
  | Headline | `.headline` | Semibold | 17 / 22 | 53 |
  | Body | `.body` | Regular | 17 / 22 | 53 |
  | Callout | `.callout` | Regular | 16 / 21 | 51 |
  | Subhead | `.subheadline` | Regular | 15 / 20 | 49 |
  | Footnote | `.footnote` | Regular | 13 / 18 | 44 |
  | Caption 1 | `.caption` | Regular | 12 / 16 | 43 |
  | Caption 2 | `.caption2` | Regular | 11 / 13 | 40 |

- [ ] **Default text is 17 pt and nothing goes below 11 pt** [HIG Typography, HIG Accessibility]. Avoid Ultralight, Thin and Light weights [HIG Typography].
- [ ] **Text scales to at least 200%** [HIG Accessibility]. Test at AX5, under Settings > Accessibility > Display & Text Size > Larger Text [HIG Typography].
- [ ] **Layouts reflow at large sizes.** Horizontal stacks become vertical, rows grow, and multi-line text isn't truncated in scroll views [HIG Layout, HIG Typography]. API: `ViewThatFits`, or `dynamicTypeSize.isAccessibilitySize` to switch `HStack` to `VStack`. No `.lineLimit(1)` on content text.
- [ ] **Custom font sizes and spacing scale too.** API: `.font(.custom("…", size: 17, relativeTo: .body))`, `@ScaledMetric` for padding and icon sizes [API]. Custom fonts must also honour Bold Text [HIG Typography].
- [ ] **Don't clamp Dynamic Type globally.** Use `.dynamicTypeSize(...DynamicTypeSize.xxxLarge)` only on chrome that truly can't grow, and never on content [API, HIG Typography].
- [ ] **Use emphasis for hierarchy, not new sizes**: `.bold()` or `.fontWeight(.semibold)` on a text style. Use as few typefaces as possible [HIG Typography].
- [ ] **Use monospace only for code and machine output** (`.monospaced()`, `.monospacedDigit()` for counters), never for labels or headers [HIG Typography: minimize typefaces].
- [ ] **Titles and buttons use title case**, such as "Add to Cart". Alert and notification body text uses sentence case [HIG Buttons, HIG Alerts, HIG Notifications].

## 5. Color and contrast

- [ ] **Use semantic system colors, not hex values.** Text uses `.primary`, `.secondary` and `Color(.tertiaryLabel)`. Backgrounds use `Color(.systemBackground)` or `Color(.systemGroupedBackground)`, and grouped lists use the grouped set [HIG Color]. Don't hard-code system color values [HIG Color].
- [ ] **Don't redefine semantic colors.** A separator color is not text, and secondary label is not a background [HIG Color].
- [ ] **Contrast meets WCAG AA** [HIG Accessibility]: 4.5:1 for text up to 17 pt, and 3:1 for 18 pt or larger or for bold text. Check it in light mode, dark mode and Increase Contrast.
- [ ] **Every custom color has light, dark and increased-contrast variants** in the asset catalog. Liquid Glass needs this even in an app with one appearance [HIG Color].
- [ ] **Color is never the only cue.** Status (running, blocked, failed, done) also has a symbol or a word [HIG Color, HIG Accessibility]. API: `Label("Blocked", systemImage: "exclamationmark.triangle.fill")`.
- [ ] **One color means one thing.** If the accent color marks tappable things, plain text never uses it [HIG Color].
- [ ] **One accent color** set through the AccentColor asset or `.tint()` at the app root. Tinted controls draw on it [HIG Color, HIG Buttons].

## 6. Touch targets, spacing and layout

- [ ] **Every hit region is at least 44×44 pt.** The absolute minimum is 28×28 pt [HIG Accessibility, HIG Buttons]. For small glyphs, use `.frame(minWidth: 44, minHeight: 44)` with `.contentShape(Rectangle())`. System buttons, list rows and toolbar items already meet the minimum.
- [ ] **Keep about 12 pt around bezeled controls and about 24 pt around borderless ones** [HIG Accessibility].
- [ ] **Margins come from the system.** Use `List` insets, `Form`, `.padding()` without a value, and `.safeAreaPadding()`. Don't use hand-picked insets that differ from screen to screen [HIG Layout: standard margins and guides].
- [ ] **Most important content goes top and leading.** Align to one leading edge and indent to show hierarchy [HIG Layout].
- [ ] **Frequent controls are reachable in the middle and bottom** of the screen, such as list swipe actions and bottom search [HIG Designing for iOS].
- [ ] **Every gesture has an on-screen alternative.** A swipe-to-dismiss also has a button, and a swipe action is also in the context menu or toolbar [HIG Accessibility, HIG Context menus].
- [ ] **Custom controls have a pressed state** [HIG Buttons]. Prefer `ButtonStyle`, which provides `configuration.isPressed`.

## 7. Buttons

- [ ] **Use system button styles only**: `.borderedProminent`, `.bordered`, `.borderless`, `.glass`, `.glassProminent` [HIG Buttons, S323]. No `Text` with `.onTapGesture` and no `RoundedRectangle` fill.
- [ ] **One or two prominent buttons per view.** Show preference through style, not size [HIG Buttons].
- [ ] **Use roles.** `role: .destructive` turns the button red. `.cancel`, `.close` and `.confirm` are system roles. Never give the primary role to a destructive action [HIG Buttons, API].
- [ ] **Labels are a verb in title case** ("Approve", "Send to Agent"), or a familiar SF Symbol with `.accessibilityLabel` [HIG Buttons].
- [ ] **Slow actions show progress inside the button**: a `ProgressView()` in the label plus a changed title, such as "Approving…" [HIG Buttons, iOS].
- [ ] **Use `.controlSize(...)` for density**, not custom frames [S323].

## 8. Lists, rows and sections

- [ ] **Show text collections in a `List`**, `.insetGrouped` for settings and detail and `.plain` for feeds [HIG Lists and tables, API `InsetGroupedListStyle`].
- [ ] **Group with `Section("Title")`** headers and footers, not hand-drawn labels. Headers are nouns in title case [HIG Lists and tables].
- [ ] **Show the row's role through its accessory.** A drill-in row uses `NavigationLink` and gets the disclosure chevron automatically. The ⓘ info button only reveals details and never navigates [HIG Lists and tables].
- [ ] **Rows are short and scannable.** Long content goes to a detail view. Truncate in the middle when both ends matter, such as paths [HIG Lists and tables]. API: `.truncationMode(.middle)`.
- [ ] **Key–value rows use `LabeledContent("Model", value: …)`**, not two `Text`s in an `HStack` with a `Spacer` [API].
- [ ] **Swipe actions use `.swipeActions(edge:allowsFullSwipe:)`.** A destructive swipe uses `role: .destructive` [API]. Every swipe action is also available somewhere visible [HIG Context menus].
- [ ] **Pull to refresh uses `.refreshable { }`**, and data also refreshes on its own [HIG Progress indicators, API].
- [ ] **Selection feedback.** Navigation rows highlight. Option rows show a checkmark [HIG Lists and tables].

## 9. Sheets, alerts, dialogs and menus

- [ ] **Sheets hold short, self-contained tasks.** Use `.sheet` with `.presentationDetents([.medium, .large])` when a half-height view is useful, plus `.presentationDragIndicator(.visible)` (the grabber) for resizable sheets [HIG Sheets, API].
- [ ] **Sheet buttons go in the right place.** Cancel or Close sits on the leading edge (`.cancellationAction`) and Done on the trailing edge (`.confirmationAction`). Done always pairs with Cancel or Back. Never show all three together [HIG Sheets].
- [ ] **Show one sheet at a time.** Never stack modals [HIG Sheets, HIG Modality].
- [ ] **Swipe-to-dismiss works.** When there are unsaved changes, confirm with a `.confirmationDialog`. API: `.interactiveDismissDisabled(hasChanges)` plus the dialog [HIG Sheets, HIG Modality].
- [ ] **Each modal has a title naming its task** and an obvious way out [HIG Modality].
- [ ] **Alerts are rare and must lead to an action.** No alerts at launch, none that only inform, and none for common undoable deletes [HIG Alerts]. API: `.alert(_:isPresented:actions:message:)`.
- [ ] **Alert titles are specific.** Never "Error" or "Error 329347". A title wraps to two lines at most, and an alert has three buttons at most. Button titles are verbs ("Delete", not "OK"). "Cancel" is always titled Cancel and is never the default [HIG Alerts].
- [ ] **Use a confirmation dialog for choices about an action the person started** (the old action sheet). API: `.confirmationDialog(_:isPresented:titleVisibility:actions:)`. Destructive items go at the top with `role: .destructive`. Cancel comes for free. The dialog must not scroll [HIG Action sheets].
- [ ] **Context menus are short and relevant.** API: `.contextMenu { } preview: { }`. Use at most about three groups and one submenu level. Hide unavailable items instead of dimming them, and put destructive items last. Every item also exists in the main UI [HIG Context menus].
- [ ] **Menu items are `Label`s with standard SF Symbols** (copy, share, trash), with icons on the leading edge [HIG Context menus, S323].

## 10. Text input and search

- [ ] **Use system `TextField` and `SecureField`**, with placeholder text and a visible label when the meaning isn't obvious [HIG Text fields]. Use `TextField(..., axis: .vertical)` for the message composer, not a custom text view.
- [ ] **Match the keyboard and autofill to the content.** API: `.keyboardType(.URL / .numberPad / .emailAddress)`, `.textContentType(...)`, `.submitLabel(.send)` [HIG Text fields, API].
- [ ] **One-time codes and pairing codes use a single `TextField`** with `.textContentType(.oneTimeCode)` and `.keyboardType(.numberPad)`, so iOS can autofill from Messages or Mail [API `oneTimeCode`]. No row of custom digit boxes.
- [ ] **Validate at the right moment.** An email is checked when the field loses focus. A username or password is checked before the person leaves the field [HIG Text fields].
- [ ] **Search uses `.searchable(text:placement:prompt:)`**, with a prompt that names what is searchable. Results update as the person types. Recent searches or suggestions come before results [HIG Search fields]. Use `.searchToolbarBehavior(.minimize)` when search is secondary [S323].
- [ ] **Scope filters use `.searchScopes`**, defaulting to the broadest scope [HIG Search fields].

## 11. Empty, loading and error states

- [ ] **Every list or screen has three states besides "full": empty, loading and error.** Use `ContentUnavailableView("No Sessions", systemImage: "…", description: Text("…"))` with an action button where one helps, and `ContentUnavailableView.search` for no results [API, HIG Tab bars: "explain why its content is unavailable"].
- [ ] **Show something immediately.** Show cached data, a skeleton made with `.redacted(reason: .placeholder)`, or a placeholder, never a blank screen [HIG Loading].
- [ ] **Use `ProgressView`.** Determinate (`ProgressView(value:)`) when the duration is known, indeterminate otherwise. Don't switch between the spinner and bar shapes. Keep it moving, keep it in one consistent place, and add Cancel when it's safe to [HIG Progress indicators].
- [ ] **Progress labels are specific**, never "Loading…" or "Authenticating…" [HIG Progress indicators].
- [ ] **Errors show inline in context.** Use a nonintrusive label with cached data, not a blocking alert. Mail's offline indicator is Apple's example [HIG Alerts]. Explain what happened and what to do next.

## 12. Notifications, Live Activities and widgets

- [ ] **Notifications are short and never repeat.** The title is in title case, the body in sentence case, and neither includes the app name or icon. No private content: set a generic `hiddenPreviewsBodyPlaceholder` such as "Approval needed" [HIG Notifications].
- [ ] **A notification has at most four actions**, each a short title-case verb with an SF Symbol. None of them just "opens the app". Mark destructive actions [HIG Notifications]. Allow and Deny on approvals fit this pattern.
- [ ] **Badges show only unread counts.** Clear them as soon as items are read, and never imitate a badge [HIG Notifications].
- [ ] **Errors go in an alert, not a notification.** In the foreground, update the UI quietly instead of notifying [HIG Notifications].
- [ ] **Live Activities track tasks with an end, up to 8 hours.** Support Lock Screen, compact, minimal and expanded. Lock Screen margins are 14 pt and heights 84–160 pt. Animations last no more than 2 s. End the activity right away when the task ends: an ended activity stays on the Lock Screen for up to 4 hours, so set a dismissal time (15–30 minutes is usually enough). Alert only for updates people must see. Tapping deep-links to the session [HIG Live activities].
- [ ] **Widgets are glanceable.** Margins are 16 pt (11 pt for tight groupings). Text is at least 11 pt and uses system text styles. Each widget has a light and a dark appearance and deep-links to the item it shows. Never show the app icon as a widget [HIG Widgets].

## 13. Onboarding, launch and permissions

- [ ] **The launch screen matches the first screen**: no text, no logo, no splash [HIG Launching].
- [ ] **Onboarding is short, interactive and skippable.** Never show it again once skipped, and keep it findable in Settings. Prefer contextual tips (TipKit) over a carousel [HIG Onboarding].
- [ ] **Ask for permissions when the feature is first used**, or in onboarding with a reason [HIG Onboarding]. Ask for notifications when the person first sets up approvals, not on first launch.
- [ ] **Start with defaults that work.** Defer configuration [HIG Onboarding].

## 14. SF Symbols and accessibility

- [ ] **Icons are SF Symbols**: `Label("Title", systemImage: "…")`, `Image(systemName:)`. They scale with Dynamic Type and match font weight [HIG SF Symbols, HIG Typography].
- [ ] **Let the container choose outline or fill.** The tab bar uses fill, the toolbar outline [HIG SF Symbols]. Use a slash variant for unavailable states.
- [ ] **Symbol animations only when they mean something**, such as `.symbolEffect(.pulse)` for "working" or `.symbolEffect(.bounce, value:)` for "done" [HIG SF Symbols, API].
- [ ] **Everything has a VoiceOver label**: `.accessibilityLabel` on icon-only buttons, and `.accessibilityElement(children: .combine)` on rows read as one item [HIG Accessibility, API].
- [ ] **Reduce Motion replaces slides and zooms with fades** [HIG Accessibility]. API: `@Environment(\.accessibilityReduceMotion)`.
- [ ] **No timed auto-dismiss** for anything people must read [HIG Accessibility].
- [ ] **Run an Accessibility Inspector audit before shipping** [HIG Accessibility].

---

## Common vibe-coded smells

Flag any of these in review. Each one has a native replacement listed above.

1. **Custom-drawn nav bar.** An `HStack` with a title and buttons at the top, instead of `NavigationStack` + `.navigationTitle` + `.toolbar`. This loses large titles, the scroll edge effect, swipe back and glass.
2. **Wordmark or app name as the screen title.** Titles name the screen and stay under 15 characters.
3. **Opaque or colored bar backgrounds**, a hand-made blur behind bars, or a divider line under the header.
4. **Glass on content.** `glassEffect` or material cards on rows, bubbles or panels, or glass stacked on glass.
5. **Hand-built buttons.** `Text` + `.onTapGesture`, `RoundedRectangle` fills, no pressed state, no role. Too many tinted buttons on one screen.
6. **Hand-built text fields.** Overlays, fake placeholders, custom caret, a custom composer box.
7. **Custom code-entry boxes** (six digit squares) instead of one `TextField` with `.textContentType(.oneTimeCode)`.
8. **Fixed font sizes.** `.font(.system(size: 13))` everywhere, `.lineLimit(1)` on content, layouts that break at AX sizes.
9. **Text below 11 pt** or light weights for "elegance".
10. **Hit targets under 44×44 pt.** Bare 16-pt icons used as buttons, or tiny chips.
11. **Uppercase monospace section labels** ("SESSIONS", "MACHINES") instead of `Section("Sessions")` headers in a `List`.
12. **Monospace or a code font used as the UI font** for anything but code and machine output.
13. **Raw IDs on screen.** UUIDs, session IDs, hashes, ports or seq numbers shown as primary text. Show human names, and put IDs behind a copy action or a debug view.
14. **Hand-built rows.** `ScrollView` + `VStack` of rounded rectangles instead of `List`. Missing chevrons, swipe actions and context menus.
15. **Key–value pairs in a hand-made `HStack`** instead of `LabeledContent`.
16. **No empty, loading or error states.** Blank screens, a lone spinner with no label, "Loading…", a raw error string.
17. **Color as the only status cue.** Green and red dots with no symbol or word, and unchecked contrast in dark mode.
18. **Hex colors** instead of semantic system colors, with no dark or increased-contrast variant.
19. **Custom back chevrons or "Back"/"Close" text** instead of the system back button and `role: .close`.
20. **Actions in the tab bar**, hidden or disabled tabs, or tabs without labels.
21. **Stacked sheets**, sheets without Cancel, Done without Cancel, swipe-to-dismiss that loses edits.
22. **Alerts for information**, alerts at launch, "Error" titles, "OK" on a destructive confirmation.
23. **Custom badges or dots** that imitate system badges.
24. **Splash screen or logo on launch**, or an onboarding carousel that can't be skipped.
25. **Emoji or custom PNG icons** where an SF Symbol exists. Icon-only buttons without an accessibility label.
26. **Hard-coded paddings that vary per screen** (13, 18, 22) instead of system insets.
27. **Decorative animation** that ignores Reduce Motion, and symbol effects with no meaning.
28. **Hand-made segmented controls or toggles** instead of `Picker(...).pickerStyle(.segmented)` and `Toggle`.

---

## Sources

HIG pages (fetched 2026-10-12 as JSON from `developer.apple.com/tutorials/data/design/human-interface-guidelines/<page>.json`):

- Designing for iOS: https://developer.apple.com/design/human-interface-guidelines/designing-for-ios
- Layout: https://developer.apple.com/design/human-interface-guidelines/layout
- Typography: https://developer.apple.com/design/human-interface-guidelines/typography
- Color: https://developer.apple.com/design/human-interface-guidelines/color
- Accessibility: https://developer.apple.com/design/human-interface-guidelines/accessibility
- Materials (Liquid Glass): https://developer.apple.com/design/human-interface-guidelines/materials
- Buttons: https://developer.apple.com/design/human-interface-guidelines/buttons
- Lists and tables: https://developer.apple.com/design/human-interface-guidelines/lists-and-tables
- Toolbars (navigation bars redirect here): https://developer.apple.com/design/human-interface-guidelines/toolbars
- Tab bars: https://developer.apple.com/design/human-interface-guidelines/tab-bars
- Sheets: https://developer.apple.com/design/human-interface-guidelines/sheets
- Modality: https://developer.apple.com/design/human-interface-guidelines/modality
- Alerts: https://developer.apple.com/design/human-interface-guidelines/alerts
- Action sheets: https://developer.apple.com/design/human-interface-guidelines/action-sheets
- Context menus: https://developer.apple.com/design/human-interface-guidelines/context-menus
- Text fields: https://developer.apple.com/design/human-interface-guidelines/text-fields
- Search fields: https://developer.apple.com/design/human-interface-guidelines/search-fields
- Notifications: https://developer.apple.com/design/human-interface-guidelines/notifications
- Live Activities: https://developer.apple.com/design/human-interface-guidelines/live-activities
- Widgets: https://developer.apple.com/design/human-interface-guidelines/widgets
- Onboarding: https://developer.apple.com/design/human-interface-guidelines/onboarding
- Launching: https://developer.apple.com/design/human-interface-guidelines/launching
- Loading: https://developer.apple.com/design/human-interface-guidelines/loading
- Progress indicators: https://developer.apple.com/design/human-interface-guidelines/progress-indicators
- SF Symbols: https://developer.apple.com/design/human-interface-guidelines/sf-symbols

The HIG has no separate pages for Liquid Glass, empty states or confirmation dialogs. Their guidance lives in Materials, Tab bars/Loading and Action sheets, plus `ContentUnavailableView` below.

WWDC25 sessions:

- 356 Get to know the new design system: https://developer.apple.com/videos/play/wwdc2025/356/
- 219 Meet Liquid Glass: https://developer.apple.com/videos/play/wwdc2025/219/
- 323 Build a SwiftUI app with the new design: https://developer.apple.com/videos/play/wwdc2025/323/

SwiftUI and UIKit reference (iOS availability checked):

- ContentUnavailableView (iOS 17): https://developer.apple.com/documentation/swiftui/contentunavailableview
- ProgressView: https://developer.apple.com/documentation/swiftui/progressview
- InsetGroupedListStyle: https://developer.apple.com/documentation/swiftui/insetgroupedliststyle
- LabeledContent: https://developer.apple.com/documentation/swiftui/labeledcontent
- ControlGroup: https://developer.apple.com/documentation/swiftui/controlgroup
- swipeActions: https://developer.apple.com/documentation/swiftui/view/swipeactions(edge:allowsfullswipe:content:)
- contextMenu(menuItems:preview:): https://developer.apple.com/documentation/swiftui/view/contextmenu(menuitems:preview:)
- presentationDetents: https://developer.apple.com/documentation/swiftui/view/presentationdetents(_:)
- presentationDragIndicator: https://developer.apple.com/documentation/swiftui/view/presentationdragindicator(_:)
- confirmationDialog: https://developer.apple.com/documentation/swiftui/view/confirmationdialog(_:ispresented:titlevisibility:actions:)
- toolbarTitleDisplayMode: https://developer.apple.com/documentation/swiftui/view/toolbartitledisplaymode(_:)
- ToolbarItemPlacement.cancellationAction: https://developer.apple.com/documentation/swiftui/toolbaritemplacement/cancellationaction
- ToolbarSpacer (iOS 26): https://developer.apple.com/documentation/swiftui/toolbarspacer
- ButtonRole.close / .confirm (iOS 26): https://developer.apple.com/documentation/swiftui/buttonrole/close, https://developer.apple.com/documentation/swiftui/buttonrole/confirm
- glassProminent button style (iOS 26): https://developer.apple.com/documentation/swiftui/primitivebuttonstyle/glassprominent
- glassEffect(_:in:) (iOS 26): https://developer.apple.com/documentation/swiftui/view/glasseffect(_:in:)
- GlassEffectContainer (iOS 26): https://developer.apple.com/documentation/swiftui/glasseffectcontainer
- scrollEdgeEffectStyle (iOS 26): https://developer.apple.com/documentation/swiftui/view/scrolledgeeffectstyle(_:for:)
- tabViewBottomAccessory (iOS 26): https://developer.apple.com/documentation/swiftui/view/tabviewbottomaccessory(content:)
- tabBarMinimizeBehavior (iOS 26): https://developer.apple.com/documentation/swiftui/view/tabbarminimizebehavior(_:)
- searchable(text:placement:prompt:): https://developer.apple.com/documentation/swiftui/view/searchable(text:placement:prompt:)
- searchToolbarBehavior (iOS 26): https://developer.apple.com/documentation/swiftui/view/searchtoolbarbehavior(_:)
- refreshable: https://developer.apple.com/documentation/swiftui/view/refreshable(action:)
- textContentType: https://developer.apple.com/documentation/swiftui/view/textcontenttype(_:)-ufdv
- UITextContentType.oneTimeCode: https://developer.apple.com/documentation/uikit/uitextcontenttype/onetimecode
- keyboardType: https://developer.apple.com/documentation/swiftui/view/keyboardtype(_:)
- submitLabel: https://developer.apple.com/documentation/swiftui/view/submitlabel(_:)
- ScaledMetric: https://developer.apple.com/documentation/swiftui/scaledmetric
- dynamicTypeSize(_:): https://developer.apple.com/documentation/swiftui/view/dynamictypesize(_:)-26aj0
- accessibilityLabel: https://developer.apple.com/documentation/swiftui/view/accessibilitylabel(_:)-1d7jv
- accessibilityElement(children:): https://developer.apple.com/documentation/swiftui/view/accessibilityelement(children:)
- symbolEffect: https://developer.apple.com/documentation/swiftui/view/symboleffect(_:options:value:)
- monospacedDigit: https://developer.apple.com/documentation/swiftui/view/monospaceddigit()
