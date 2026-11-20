# Reference Results

These are the original runs used for the graphs and summary tables.

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
- Notes:

```
[[trusted_validators]]
jsonrpc_address = "127.0.0.1:8232"
connect_timeout_secs = 2
read_timeout_secs = 30
max_connections = 32
max_response_mib = 32
priority = 0

[serve]
grpc_listen_address = "127.0.0.1:8137"
grpc_web = false
max_address_rows = 100000
state_dir = "/home/myusername/.local/share/zingod/state"
hold_goaway_delay_secs = 0
tls = "off"

[grpc]
max_connections = 1024
max_connections_per_ip = 512
max_streams_per_connection = 100
max_work_streams = 1024
max_subscriptions = 1024
max_point_reads = 96
max_range_reads = 192
max_scan_reads = 8
unread_timeout_secs = 300
silent_timeout_secs = 300
max_connection_age_secs = 1800
max_connection_age_grace_secs = 600
max_connection_idle_secs = 1800
adaptive_work_streams = true
trusted_proxies = []
shutdown_grace_secs = 10
```


