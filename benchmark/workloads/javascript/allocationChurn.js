let total = 0;
for (let index = 1; index <= 20_000; index += 1) {
  const values = new Array(256).fill(index);
  total += values[0];
}
console.log(total);
