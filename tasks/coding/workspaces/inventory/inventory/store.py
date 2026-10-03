"""Stock levels and reservations."""


class InsufficientStock(Exception):
    """Raised when a removal or reservation asks for more than is available."""

    def __init__(self, sku, requested, available):
        self.sku = sku
        self.requested = requested
        self.available = available
        super().__init__(f"insufficient stock for {sku}: requested {requested}, available {available}")


class Inventory:
    def __init__(self):
        raise NotImplementedError

    def add(self, sku, qty):
        raise NotImplementedError

    def remove(self, sku, qty):
        raise NotImplementedError

    def stock(self, sku):
        raise NotImplementedError

    def skus(self):
        raise NotImplementedError
