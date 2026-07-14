const values = new Float64Array(1_000_000);
for (let index = 0; index < values.length; index += 1) values[index] = index + 1;
let total = 0;
for (let index = 0; index < values.length; index += 1) total += values[index];
console.log(total);
