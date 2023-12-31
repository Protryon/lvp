
# lvp: Local Volume CSI Provisioner

LVP is a simple K8S CSI provisioner for provisioning simple local volumes in a small cluster.

## Motivation

I got tired of
* Poor support from things like helm charts for static provisioning
* Defining my PersistentVolumes by hand
* Having my cluster node hostnames handwritten in deployment manifests

...so I wrote a CSI provisioner.

## Installing

I recommend installing via the Helm chart.

```
helm upgrade --install lvp oci://registry-1.docker.io/protryon/lvp \
  --version 1.2.0-helm --namespace lvp --create-namespace
```

Or directly from this git:
```
helm upgrade --install lvp ./charts/lvp \
  --version 1.2.0-helm --namespace lvp --create-namespace
```

## Limitations

* I haven't had a multi-node cluster to test this with multiple nodes, but I'm fairly sure it'll need some minor tweaks. If someone wants to buy me another (nice) old server for my garage, then we can talk. Notably, we probably need the ability to derive the node name from the node somehow, as opposed to configuring it.

## Capabilities

* Dynamic provisioning
* Create loop mounted `ext4` and `xfs` volumes
* Create bind mounted volumes
* Volume resizing for `ext4` and `xfs` volumes
* Most of the CSI spec, so stuff like K8S volume metrics and capacity tracking (except for bind mounts)

## Stuff To Do

* Add proper support for running more than one K8s node (or at least having volumes on more than one node)