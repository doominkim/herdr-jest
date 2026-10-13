const { checkout } = require('./checkout');

describe('Checkout', () => {
  describe('shipping', () => {
    it('is free over 50', () => {
      expect(checkout([{ price: 60, qty: 1 }]).shipping).toBe(0);
    });

    it('costs 5 under 50', () => {
      expect(checkout([{ price: 10, qty: 2 }]).shipping).toBe(5);
    });
  });

  describe('coupons', () => {
    it('takes 10% off with WELCOME10', () => {
      expect(checkout([{ price: 100, qty: 1 }], 'WELCOME10').total).toBe(90);
    });

    it('ignores unknown coupons', () => {
      expect(checkout([{ price: 100, qty: 1 }], 'NOPE').total).toBe(100);
    });
  });
});
