function withTax(amount, rate) {
  return Math.round(amount * (1 + rate));
}

module.exports = { withTax };
