"""Stock levels and reservations."""


class InsufficientStock(Exception):
    """Raised when a removal or reservation asks for more than is available."""

    def __init__(self, sku, requested, available):
        self.sku = sku
        self.requested = requested
        self.available = available
        super().__init__(f"insufficient stock for {sku}: requested {requested}, available {available}")


def _check_qty(qty):
    if not isinstance(qty, int) or isinstance(qty, bool) or qty <= 0:
        raise ValueError(f"quantity must be a positive integer, got {qty!r}")


class Inventory:
    def __init__(self):
        self._stock = {}
        self._reserved = {}  # order_id -> {sku: qty}

    def add(self, sku, qty):
        """Receive qty units of sku; qty must be a positive integer."""
        _check_qty(qty)
        self._stock[sku] = self._stock.get(sku, 0) + qty

    def remove(self, sku, qty):
        """Ship qty units of sku out of the unreserved stock, or raise
        InsufficientStock (an unknown sku has 0 available)."""
        _check_qty(qty)
        have = self.available(sku)
        if qty > have:
            raise InsufficientStock(sku, qty, have)
        self._stock[sku] = self._stock.get(sku, 0) - qty

    def reserved(self, sku):
        """Units of sku held by open reservations."""
        return sum(r.get(sku, 0) for r in self._reserved.values())

    def available(self, sku):
        """Stock on hand minus open reservations."""
        return self.stock(sku) - self.reserved(sku)

    def reserve(self, order_id, sku, qty):
        """Hold qty units of sku for an order. Reserving the same sku again
        for the same order replaces the earlier quantity. Raises
        InsufficientStock against what is available once the order's own
        earlier hold on that sku is released."""
        _check_qty(qty)
        own = self._reserved.get(order_id, {}).get(sku, 0)
        free = self.available(sku) + own
        if qty > free:
            raise InsufficientStock(sku, qty, free)
        self._reserved.setdefault(order_id, {})[sku] = qty

    def release(self, order_id):
        """Drop every hold of an order; unknown orders are ignored."""
        self._reserved.pop(order_id, None)

    def fulfil(self, order_id):
        """Ship an order's holds and release it; KeyError for an unknown order."""
        holds = self._reserved.pop(order_id)
        for sku, qty in holds.items():
            self._stock[sku] = self._stock.get(sku, 0) - qty

    def stock(self, sku):
        """Units on hand; 0 for an unknown sku."""
        return self._stock.get(sku, 0)

    def skus(self):
        """Sorted skus with stock on hand."""
        return sorted(s for s, q in self._stock.items() if q > 0)
