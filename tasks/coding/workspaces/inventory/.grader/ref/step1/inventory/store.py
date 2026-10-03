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

    def add(self, sku, qty):
        """Receive qty units of sku; qty must be a positive integer."""
        _check_qty(qty)
        self._stock[sku] = self._stock.get(sku, 0) + qty

    def remove(self, sku, qty):
        """Ship qty units of sku, or raise InsufficientStock (an unknown sku
        has 0 available)."""
        _check_qty(qty)
        have = self._stock.get(sku, 0)
        if qty > have:
            raise InsufficientStock(sku, qty, have)
        self._stock[sku] = have - qty

    def stock(self, sku):
        """Units on hand; 0 for an unknown sku."""
        return self._stock.get(sku, 0)

    def skus(self):
        """Sorted skus with stock on hand."""
        return sorted(s for s, q in self._stock.items() if q > 0)
