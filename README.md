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

Assuming you want to run `cgroup-memory-manager` on a Kubernetes node, you can use the following
systemd unit file:


```
[Unit]
Description=cgroup-memory-manager
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/cgroup-memory-manager \
    --parent /sys/fs/cgroup/memory/kubepods \
    --threshold 25%
Restart=always

[Install]
WantedBy=multi-user.target
```

# Running as DaemonSet in Kubernetes

Example of DaemonSet manifest for cgroup-memory-manager that mounts `/sys/fs/cgroup` from the host
to a container:

```yaml
apiVersion: extensions/v1beta1
kind: DaemonSet
metadata:
  name: cgroup-memory-manager
  namespace: kube-system
spec:
  revisionHistoryLimit: 10
  selector:
    matchLabels:
      app: cgroup-memory-manager
  updateStrategy:
    type: RollingUpdate
    rollingUpdate:
      maxUnavailable: 100%
  template:
    metadata:
      labels:
        app: cgroup-memory-manager
    spec:
      containers:
        - name: cgroup-memory-manager
          image: linchpiner/cgroup-memory-manager
          imagePullPolicy: IfNotPresent
          command:
            - cgroup-memory-manager
            - --parent=/host/sys/fs/cgroup/memory/kubepods
            - --threshold=25%
          env:
            - name: RUST_LOG
              value: info
          resources:
            limits:
              cpu: 300m
              memory: 512Mi
          volumeMounts:
            - mountPath: /host/sys/fs/cgroup
              name: sysfs
      restartPolicy: Always
      volumes:
        - hostPath:
            path: /sys/fs/cgroup
            type: ""
          name: sysfs
```

# Details

In Kubernetes, cgroups for container in Pods have complex hierarchy that includes Pod QoS class, for
example:

```
/sys/fs/cgroup/memory/kubepods/
├── podA
│   └── containerA1
├── burstable
│   └── podB
│       └── containerB1
└── besteffort
    └── podC
        └── containerC1
```

While it is possible to monitor memory consumption for the parent cgroups that correspond to Pods or
QoS classes, `cgroup-memory-manager` does this only for cgroups that correspond to containers.
For Kubernetes, set `--parent` to `/sys/fs/cgroup/memory/kubepods`.

Only the cgroup memory resource controller v1 is supported, see
https://www.kernel.org/doc/Documentation/cgroup-v1/memory.txt.
