# What is it?

This program periodically scans all the child cgroups of the specified parent cgroup and analyzes
memory consumption using control file `memory.stat` of the cgroup memory resource controller.
When cgroup cache usage is higher than the specified threshold, it triggers a forced page reclaim
via control file `memory.force_empty`, but not more often than once in the specified time frmae.

# Why do you need it?

Linux cgroup-aware out-of-memory (OOM) killer accounts RSS, kmem, and cache when calculating memory usage for a cgroup.
A process that is running in a cgroup cannot directly control its cache usage.

It is a good practice in Kubernetes to set a memory limit for containers.
However, even if your program does not consume more than the limit, OOM killer can kill your
container if the total usage (RSS+cache) is bigger than the limit. 

# Usage

`cgroup-memory-manager [OPTIONS]`

Options:
- `--parent` path to the parent cgroup
- `--threshold` cache usage threshold in %, bytes, or other units
- `--interval` how frequently to check cache usage for all cgroups in seconds
- `--cooldown` the minimum time to wait in seconds between forcing page reclaim

Set environment variable `RUST_LOG=info` to see what cgroups are detected and reclaimed.

# Running as a process on host

TODO

# Running as DaemonSet in Kubernetes

TODO

# Details

In Kubernetes, cgroups for container in Pods have complex hierarchy that includes Pod QoS class, for
example:

```
/sys/fs/cgroup/memory/kubepods/
├── podA
│   ├── containerA1
│   └── containerA2
├── podB
│   ├── containerB1
│   └── containerB2
├── burstable
│   ├── podC
│   │   ├── containerC1
│   │   └── containerC2
│   └── podD
│       ├── containerD1
│       └── containerD2
└── besteffort
    ├── podE
    │   ├── containerE1
    │   └── containerE2
    └── podF
        ├── containerF1
        └── containerF2
```

While it is possible to monitor memory consumption for the parent cgroups that correspond to Pods or
QoS classes, `cgroup-memory-manager` does this only for cgroups that correspond to containers.

Only the cgroup memory resource controller v1 is supported, see
https://www.kernel.org/doc/Documentation/cgroup-v1/memory.txt for the details.
