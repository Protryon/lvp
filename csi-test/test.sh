#!/bin/bash

./csi-sanity --csi.endpoint=unix:///csi/csi.sock -csi.testvolumeparameters ./parameters.yaml -csi.mountdir /csi/test "$@"