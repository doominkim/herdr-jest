function checkout(cart, coupon) {
  const subtotal = cart.reduce((acc, item) => acc + item.price * item.qty, 0);
  const shipping = subtotal >= 50 ? 0 : 5;
  const discount = coupon === 'WELCOME10' ? subtotal * 0.1 : 0;
  return { subtotal, shipping, total: subtotal + shipping - discount };
}

module.exports = { checkout };
