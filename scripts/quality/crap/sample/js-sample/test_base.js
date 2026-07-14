const assert = require('assert');
const { simpleAdd, gnarly } = require('./index.js');
assert.strictEqual(simpleAdd(2,3),5);
// exercise gnarly across branches so it is covered in the BASE snapshot
gnarly(3,4,0); gnarly(3,-20,1); gnarly(3,0,2); gnarly(-9,0,0); gnarly(-9,0,1);
gnarly(-9,0,2); gnarly(-9,0,3); gnarly(-9,0,9); gnarly(12,1,0); gnarly(-12,0,0);
console.log('base tests passed');
