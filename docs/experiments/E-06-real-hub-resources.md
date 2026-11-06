# E-06: Blazor hub CPU and RAM

- Status: **to do**

**Question:** What does the real hub cost with 100 machines and 20 browser viewers?

**How:** docker compose up the hub. Run klark-loadtest --machines 100 with a fleet token. Open 20 browser tabs on panes with Playwright. Record docker stats every 5 s for 5 minutes.

**Pass / decision rule:** Below 1 vCPU and 512 MB RAM. If over, check: per-viewer fan-out (S-13), the cost of a SignalR circuit, and JSON parsing of trees.
