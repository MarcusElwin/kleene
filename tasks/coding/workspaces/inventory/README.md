# inventory

Stock tracking for a small warehouse: `inventory/store.py` holds the
`Inventory` class (stock, reservations, reports) and `inventory/events.py`
replays event logs into it. Tests live under `tests/` and run with
`python3 -m unittest`.
