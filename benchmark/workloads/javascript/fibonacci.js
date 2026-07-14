function fibonacci(value) {
  if (value < 2) return value;
  return fibonacci(value - 1) + fibonacci(value - 2);
}

let total = 0;
for (let index = 0; index < 30; index += 1) total += fibonacci(28);
console.log(total);
