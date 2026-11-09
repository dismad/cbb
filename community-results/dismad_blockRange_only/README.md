## Machine
- CPU: Intel Core Ultra 9 185H (16 cores / 16 threads, up to 5.1 GHz)
- RAM: 30 GiB
- OS: Ubuntu 26.04.1 LTS
- Kernel: 7.0.0-42-generic
- Zebra version: v7.0.0+git.e8db87e ( w' indexed flag )
- Indexer versions:
  - lightwalletd: v0.6.1 (commit 0bf0a40854de6bd9a50400f94f0f94fbb53ac743, built 2026-11-07 by zktails)
  - zainod: 0.10.1
  - zingod: 1.0.0
  - ztreamerd 0.1.0
- Notes: Ztreamerd is running in embedded archive mode (used zakura-mainnet-20261106T010818Z-3543337.tar.zst )

Only tested BlockRange to isolate its performance. Left out zebrad native due to its much slower speed. 