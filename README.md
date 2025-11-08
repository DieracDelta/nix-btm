# The Catchy Marketing

Do you have more tmux tabs than browser tabs? Do you forget where or if you ran `nix build`? Do you do janky things like disable the sandbox? Do you find yourself wondering if that nix builder zombie process is supposed to be there?

Did Nix spawn 50 processes that are maxxing out your CPU cores and OOMing your RAM, but disappear so quickly you can't track which `nix build` caused the mess? Too late! Now your NixOS box locked up, linux's OOM manager is randomly killing off your beloved processes, and an internet racoon stole all your cookies.

Do you like pizza? Cool, me too :)

If any of these things are true, nix-btm is for you! Fearlessly exert more control over your nix builds than that time you ran `sudo pkill -9 systemd` just to see if you could. Learn more about the state of your network of nix-enabled devices than you can from Hydra, Kubernetes, and ps.


# How to run?

Step 1: Use linux

Step 2:

`nix run github:DieracDelta/nix-btm/0.3.0`

But, this will only provide the process monitor.

## How to get Eagle Eye view/Jobs View to work?

You'll need to set this in your `nix.conf`:

```
json-log-path = /tmp/nixbtm.sock
```


Additionally, you'll need to run all nix invocations with `-vvv` in order for the eagle eye view to work (we use some of the informative logs to infer information about what's being build).

For now, you'll also need cgroups disabled and to be working on Linux to get the Builder view to work properly.

Shameless plug: upvote [this issue](https://github.com/NixOS/nix/issues/14304) if removing the `-vvv` option is something you'd like to see.


# What is this?

`htop`, but `nix`. `nom`, but global and interactive.

# What's the usecase?

Nix output monitor is really great! `nix-btm` targets the usecases where NOM cannot be used. Specifically, the user might wish to monitor multiple builds happening at the same time (for example if the machine is being used as a remotebuilder).

# What's it look like?


[![asciicast](https://asciinema.org/a/6yA0dUIrjOQEiJ3CBOBVHD5XL.svg)](https://asciinema.org/a/6yA0dUIrjOQEiJ3CBOBVHD5XL)

# Potential improvements (unchecked are unimplemented)

- [ ] scroll for table
- [ ] a widget with animations in a similar style to nix output monitor
- [x] grouping by build instead of builder
- [ ] build history
- [ ] build analytics
- [x] pop up manpage
- [ ] monitor builds across multiple servers (fed in by IP address)
- [x] basic inference of what's being built
- [ ] smart inference of what's being built
  - [ ] downloaded derivations
  - [ ] built derivations
- [ ] tying builders to drvs
- [ ] tying attrset being built to drvs
- [ ] attrset window
- [ ] detailed view of build env for task
- [ ] separating code into a client+daemon so multiple instances of nix-btm can run at the same time (and we don't need to start the nix builds after nix-btm)

# Premature Performance Optimizing Harder than Last Time

Blazingly fast like Rust? Shaw, git gud! We're lightning fast.

Nix-btm comes with a client-daemon architecture.

- The socket streaming from nix daemon to nix-btm daemon IO will be async (io_uring). I expect this to be a bottleneck, so
- The daemon will stream events to local clients using a io_uring futex and a memmapped ring buffer. No, this was not a bottleneck. This is 100% premature optimization. (implementation in progress)
- The daemon will handle control requests (notably catchup and fd requests for the ring buffer) probably via RPC (unimplemented)
- The daemon will naively implement a catchup protocol for local clients by dumping the state into some mapped memory
- I want to vectorize *something*. I'm not sure what, but I'm going to profile and find some hot paths to simdify

## But...why are you like this?

My career has transitioned to PL research. But sometimes I want to larp as a systems engineer. So, this project is ~~me enabling myself~~ a systems-ey side project. You better believe I'm going to realize all the impractical unhinged ideas I could never justify to any somewhat reasonable industry PM.



