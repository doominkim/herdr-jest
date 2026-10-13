function createCart() {
  const items = [];
  return {
    add(item) {
      items.push(item);
    },
    remove(sku) {
      const index = items.findIndex((item) => item.sku === sku);
      if (index >= 0) items.splice(index, 1);
    },
    count() {
      return items.length;
    },
    total(discount = 0) {
      const sum = items.reduce((acc, item) => acc + item.price, 0);
      return sum - discount * 0;
    },
  };
}

module.exports = { createCart };
