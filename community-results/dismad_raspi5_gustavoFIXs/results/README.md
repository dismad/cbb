# Reference Results

These are the original runs used for the graphs and summary tables.

## Machine


- **OS:** Ubuntu 26.04.1 LTS aarch64  
- **Host:** Raspberry Pi 5 Model B Rev 1.0  
- **Kernel:** 6.17.0-1017-raspi  
- **Uptime:** 29 days, 1 hour, 1 min  
- **Packages:** 2273 (dpkg), 22 (snap)  
- **Shell:** bash 5.3.9  
- **Resolution:** 3440x1440  
- **Terminal:** /dev/pts/0  
- **CPU:** (4) @ 2.400GHz  
- **Memory:** 4089MiB / 7930MiB

- cbb latest upstream with gustavo's fixes merged
- lwd 0.60
- zingod 1.1.0


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
adaptive_work_streams = false
trusted_proxies = []
shutdown_grace_secs = 10

[grpc.overload]
cpu_pressure_percent = 0
io_pressure_percent = 0

```


