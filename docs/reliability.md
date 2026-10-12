# Reliability requirements

Each requirement below is pass/fail with an automated test, and a failing test blocks a release. The numbers are targets to tune after the phase 0 spikes.

| Requirement | Pass condition | Test |
| --- | --- | --- |
| Long sessions stay light | A 100,000-event session opens in under 50 ms from cache; daemon memory per idle session stays flat | Scripted long session in CI, memory sampled every turn |
| The UI never lags | 1,000 sessions in the sidebar scroll at full frame rate; no frame over 16 ms during streaming | Scripted UI run with frame-time capture |
| Status is never wrong | A session shows working only while its agent process is producing events; asleep and offline machines are labelled as such | Kill, freeze and sleep the agent and the machine; compare shown status to reality |
| Approvals answered once, everywhere | An approval answered on one device disappears from all others within 1 second | Two clients and a phone simulator answering at once |
| Stop means stopped | The agent process has exited, or shows "stopping" with the reason, within 2 seconds | Stop during tool calls, subagents and long shell commands |
| Survives sleep and network loss | After 10 minutes offline or a closed lid, the session continues and clients resume with no action from you | Network partition and suspend tests on each platform |
| Survives restarts | Killing the daemon or client mid-turn loses no events; the turn is marked interrupted and can continue in one click | Forced kills at random points in a turn |
| Mobile stays fast | A 10,000-message chat opens in under 300 ms on a three-year-old iPhone | Device test in CI |
| Approval taps always land | A tap on allow or deny shows the daemon's acknowledgement within 500 ms, or a visible error | Approval tests over slow and lossy connections |
| No lock-in surprises | Local use works with no sign-in; dent uses the agent CLIs you installed | Fresh-machine install test without an account |
| Encrypted end to end | The hub's database and logs contain no plaintext session content | Hub test that scans stored data for planted strings |
