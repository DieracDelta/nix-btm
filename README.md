# Catchy Marketing

Do you have more tmux tabs than browser tabs? Do you forget where or if you ran `nix build`? (You might have ADHD, you should probably get that checked)

Did Nix spawn 50 processes that are maxxing out your CPU cores and OOMing your RAM, but disappear so quickly you can't track which `nix build` caused this mess? Too late! Now your NixOS box locked up, linux's OOM manager is randmoly killing off your beloved browser tabs, and an internet racoon stole all your cookies.

Do you like pizza? Cool, me too :)

If any of these things are true, nix-btm is for you! Fearlessly exert more control over your nix builds than that time you ran `sudo pkill -9 systemd` just to see if you could.


# How to run?

`nix run github:DieracDelta/nix-btm/master`

# How to get Eagle Eye view/Jobs View to work?

You'll need to set this in your `nix.conf`:

```
json-log-path = /tmp/nixbtm.sock
```


Additionally, you'll need to run all nix invocations with `-vvv` in order for the eagle eye view to work (we use some of the informative logs to infer information about what's being build).

Shameless plug: upvote [this issue](https://github.com/NixOS/nix/issues/14304) if removing the `-vvv` option is something you'd like to see.


# What is this?

`nix-btm` is intended to be the spiritual successor of `nix-top`, which has been recently deleted.

As it stands currently, `nix-btm` has feature parity with `nix-top` on Linux. On Macos, feature parity is reached if run as root.

# What's the usecase?

Nix output monitor is really great! `nix-btm` targets the usecases where NOM cannot be used. Specifically, the user might wish to monitor multiple builds happening at the same time (for example if the machine is being used as a remotebuilder).

# What's it look like?

TODO update this

[![asciicast](https://asciinema.org/a/hG1DHrV4RC6bF0W1oO1GqvlIb.svg)](https://asciinema.org/a/hG1DHrV4RC6bF0W1oO1GqvlIb)

# Potential improvements (unchecked are unimplemented)

- [ ] scroll for table
- [ ] a widget with animations in a similar style to nix output monitor
- [ ] grouping by build instead of builder (not sure if this *is* possible but we might be able to grep)
- [ ] build history
- [ ] build analytics
- [x] pop up manpage
- [ ] monitor builds across multiple servers (fed in by IP address)
- [ ] inference of what's being built
- [ ] tree view by pid parent
- [ ] detailed view of build env for task

# What are some cool things we're doing?

Blazingly fast? Shaw, git gud! We're lightning fast.

Nix-btm comes with a client-daemon architecture that will stream events to client using io_uring and a shared memory ring buffer as our IPC. No, this was not a bottleneck. This is 100% premature optimization.

