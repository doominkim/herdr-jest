const { formatPrice } = require('./format');

describe('formatPrice', () => {
  it('formats cents as dollars', () => {
    expect(formatPrice(1999)).toBe('$19.99');
  });

  it('pads whole dollars', () => {
    expect(formatPrice(500)).toBe('$5.00');
  });
});
