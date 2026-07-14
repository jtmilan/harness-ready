// Tiny JS sample for the fallow CRAP self-test.
// simpleAdd is trivial + tested. gnarly is high-complexity + untested.

function simpleAdd(a, b) {
  return a + b;
}

function gnarly(x, y, mode) {
  let acc = 0;
  if (x > 0) {
    if (y > 0) {
      acc += x * y;
    } else if (y < -10) {
      acc -= x;
    } else {
      acc += 1;
    }
  } else if (x < -5) {
    switch (mode) {
      case 0: acc += 1; break;
      case 1: acc += 2; break;
      case 2: acc += 3; break;
      case 3: acc += 4; break;
      default: acc -= 1;
    }
  }
  for (let i = 0; i < Math.abs(x); i++) {
    if (i % 2 === 0 && i % 3 === 0) {
      acc += i;
    } else if (i % 5 === 0 || i % 7 === 0) {
      acc -= i;
    }
  }
  while (acc > 100) {
    acc = Math.floor(acc / 2);
  }
  return acc;
}

module.exports = { simpleAdd, gnarly };
