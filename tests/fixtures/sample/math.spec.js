describe('math', () => {
  it('adds', () => {
    expect(1 + 1).toBe(2);
  });

  it('multiplies wrong', () => {
    expect(2 * 2).toBe(5);
  });

  it.skip('skipped one', () => {});

  it.each([
    [1, 1, 2],
    [2, 2, 4],
  ])('sums %i + %i', (a, b, sum) => {
    expect(a + b).toBe(sum);
  });
});
