# Remote machines

Every machine runs the same daemon, so a Mac server, a Linux box and a throwaway EC2 instance differ only in how they are installed and how long they live.

## Install and upgrade

- One-line install script, plus Homebrew, a Linux package and a Windows installer.
- `dent daemon install` registers the system service, then opens a sign-in link, or prints one on a headless machine.
- The daemon checks that the agent CLIs it needs are installed and logged in, and reports what is missing to every client.
- Self-update with staged rollout across machines; clients and daemons negotiate protocol versions, so an older daemon keeps working with a newer client.

## Always-on machines

- On macOS the daemon can hold a sleep assertion while any agent is working, so a closed-lid laptop or a Mac server keeps going.
- Each machine tells paired devices what it is (OS and version, chip and cores, dent version, agent CLIs and versions, network hardware addresses for Wake-on-LAN, time zone) and, every 5 seconds while watched, how it's doing (load, memory, disk, battery, agents running). Unpaired devices learn none of it. Shown in a machine's details in Settings › Devices ([spike 05](spikes/05-direct-links.md#what-each-machine-reports)).

## Ephemeral machines

Machine lifecycle goes through a provider interface: launch, wait until ready, destroy, and list.

- **Command provider:** you supply commands for each step that print JSON, which plugs in your existing EC2 tooling unchanged.
- **AWS provider:** a built-in one using the AWS SDK for Rust, for public users.
- Launch passes a short-lived join token from the hub through instance user data, so a new machine installs the daemon and joins your account with no sign-in.
- Before teardown, dent waits for at least one other device to hold the machine's full session logs and synced docs, then destroys it. Its sessions stay readable as archived.
- Optional idle timeout tears machines down when no session has been active for a set time.
- Agent logins on fresh machines come from the provider's bootstrap step, never from dent copying credentials.
- **Hosted provider (paid):** the hub's own cloud machines, for users with no cloud account; billed by usage.
- **Fleet updates:** the hub announces new daemon versions, and one button updates every machine.
