# Portable HEVC sources

Pinned unmodified Kvazaar v2.3.2 and libde265 v1.1.3 source releases from their
upstream GitHub repositories. Original copyright and license files are retained.
The library and CLI have separate licenses: libde265's library is LGPLv3; its
GPL CLI is not built or linked. Kvazaar is BSD-3-Clause.

`vnilink-hevc` builds copies in Cargo's OUT_DIR, never this source tree. Its shim
owns contexts and bounds packets, allocations and image geometry. Kvazaar global
strategy dispatch is serialized across owners; gop=0/owf=0 completes each encode
before another owner can change dispatch. Each stream keeps separate references.
libde265 uses a bounded allocation callback and resets after a rejected packet.
Both codecs use 8-bit I420, with no subprocess on the data path.

Static LGPL relinking materials are included in the Electron output. See
assets/licenses/native-codecs.txt. These materials must also remain included
when a production installer is introduced.
