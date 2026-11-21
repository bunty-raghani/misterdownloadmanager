# IDM 6, 42, 25, 3 vs MDM caa200c — 2026-10-29T11:30:33

13th Gen Intel(R) Core(TM) i5-13400F, 63.8 GB RAM, Windows-11-10.0.26200-SP0. labserver (this repo), 127.0.0.1:8097, generated content. both at defaults: 8 connections.

| scenario | IDM median ms (IQR) | MDM median ms (IQR) | IDM / MDM (95% CI) | p (Mann-Whitney) |
|---|---|---|---|---|
| 1 GiB over a fast local link | 5456 (5393–5496) | 1009 (944–1028) | 5.41× (5.21–5.89) | 0.000583 |
| 100 MiB, 2 MB/s per connection | 10234 (10221–10284) | 6319 (6317–6329) | 1.62× (1.61–1.63) | 0.000583 |
| 100 MiB, 150 ms latency, 4 MB/s per connection | 7622 (7497–7863) | 3844 (3842–3894) | 1.98× (1.92–2.05) | 0.000583 |
| 32 MiB, one connection in three crawls at 128 KB/s | 7213 (6960–7256) | 3432 (3416–3840) | 2.10× (1.81–2.13) | 0.000583 |
| 50 MiB, server allows 2 connections, 2 MB/s each | 15864 (15810–15933) | 12568 (12563–12570) | 1.26× (1.25–1.27) | 0.000583 |
| 50 MiB, every 2nd connection cut after 2 MiB, 4 MB/s | 5428 (5387–5466) | 1647 (1645–1660) | 3.30× (3.24–3.34) | 0.000583 |
| 256 KiB file over a fast local link | 3141 (3130–3164) | 30 (28–31) | 105.90× (99.96–112.75) | 3.39e-06 |

Phases (median ms): startup / transfer / finish

- fast-1g IDM: 3076 / 1831 / 545; connections 9.0, requests 9.0, CPU 2390.6 ms, peak memory 46.3 MB
- fast-1g MDM: 14 / 438 / 556; connections 17.0, requests 31.0, CPU 1765.6 ms, peak memory 106.2 MB
- capped-100m IDM: 3081 / 7068 / 84; connections 9.0, requests 9.0, CPU 734.4 ms, peak memory 46.6 MB
- capped-100m MDM: 14 / 6299 / 14; connections 12.0, requests 17.0, CPU 281.2 ms, peak memory 100.4 MB
- latency-100m IDM: 3073 / 4451 / 67; connections 8.0, requests 8.0, CPU 578.1 ms, peak memory 46.7 MB
- latency-100m MDM: 14 / 3801 / 32; connections 17.0, requests 24.0, CPU 265.6 ms, peak memory 101.9 MB
- slowconn-32m IDM: 3063 / 4072 / 36; connections 20.0, requests 20.0, CPU 375.0 ms, peak memory 46.7 MB
- slowconn-32m MDM: 13 / 3410 / 8; connections 15.0, requests 32.0, CPU 250.0 ms, peak memory 100.5 MB
- maxconns-50m IDM: 3069 / 12733 / 48; connections 480.0, requests 480.0, CPU 1046.9 ms, peak memory 47.8 MB
- maxconns-50m MDM: 14 / 12546 / 9; connections 2.0, requests 2.0, CPU 390.6 ms, peak memory 96.4 MB
- drops-50m IDM: 3082 / 2280 / 50; connections 15.0, requests 15.0, CPU 453.1 ms, peak memory 47.8 MB
- drops-50m MDM: 14 / 1604 / 27; connections 19.0, requests 26.0, CPU 203.1 ms, peak memory 102.1 MB
- small-256k IDM: 3090 / 25 / 32; connections 4.0, requests 4.0, CPU 156.2 ms, peak memory 43.2 MB
- small-256k MDM: 14 / 7 / 8; connections 4.0, requests 6.0, CPU 93.8 ms, peak memory 98.2 MB
