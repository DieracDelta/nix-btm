# How to run?

`nix run github:DieracDelta/nix-btm/master`

# What is this?

`nix-btm` is intended to be the spiritual successor of `nix-top`, which has been recently deleted.

As it stands currently, `nix-btm` has feature parity with `nix-top` on Linux. On Macos, feature parity is reached if run as root.

# What's the usecase?

Nix output monitor is really great! `nix-btm` targets the usecases where NOM cannot be used. Specifically, the user might wish to monitor multiple builds happening at the same time (for example if the machine is being used as a remotebuilder).

# What's it look like?

https://asciinema.org/a/hG1DHrV4RC6bF0W1oO1GqvlIb
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


