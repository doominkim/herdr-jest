const { isValidEmail } = require('./user');

describe('User signup', () => {
  it('accepts a valid email', () => {
    expect(isValidEmail('kim@acme.dev')).toBe(true);
  });

  it('rejects an email without a domain', () => {
    expect(isValidEmail('kim@')).toBe(false);
  });

  it.skip('sends a welcome email', () => {});

  it.todo('locks the account after five failed logins');
});
