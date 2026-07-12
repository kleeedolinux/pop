package main

import "fmt"

func fibonacci(value uint64) uint64 {
	if value < 2 {
		return value
	}
	return fibonacci(value-1) + fibonacci(value-2)
}

func main() {
	var total uint64
	for index := 0; index < 30; index++ {
		total += fibonacci(28)
	}
	fmt.Println(total)
}
