const { withTax } = require('./price');

describe('price', () => {
  it('adds 10% tax', () => {
    expect(withTax(100)).toBe(110);
  });
});
