class Box {
  constructor(value) {
    this.value = value;
  }
}

const values = new Array(200_000);
for (let index = 0; index < values.length; index += 1) values[index] = new Box(index + 1);
let total = 0;
for (let index = 0; index < values.length; index += 1) total += values[index].value;
console.log(total);
