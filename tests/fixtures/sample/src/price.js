function withTax(amount) {
  return Math.round(amount * 1.1);
}

module.exports = { withTax };
