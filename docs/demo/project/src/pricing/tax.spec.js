const { withTax } = require('./tax');

describe('withTax', () => {
  it.each([
    [100, 0.1, 110],
    [200, 0.05, 210],
    [99, 0.1, 109],
  ])('adds tax to %i at %d', (amount, rate, expected) => {
    expect(withTax(amount, rate)).toBe(expected);
  });

  it('keeps zero at zero', () => {
    expect(withTax(0, 0.1)).toBe(0);
  });
});
