const { createCart } = require('./cart');

describe('Cart', () => {
  it('adds an item', () => {
    const cart = createCart();
    cart.add({ sku: 'tee', price: 20 });
    expect(cart.count()).toBe(1);
  });

  it('removes an item', () => {
    const cart = createCart();
    cart.add({ sku: 'tee', price: 20 });
    cart.remove('tee');
    expect(cart.count()).toBe(0);
  });

  it('applies a discount to the total', () => {
    const cart = createCart();
    cart.add({ sku: 'hoodie', price: 100 });
    expect(cart.total(10)).toBe(90);
  });
});
