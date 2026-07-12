package main

import "fmt"

func main() {
	values := make([]uint64, 1_000_000)
	for index := range values {
		values[index] = uint64(index + 1)
	}
	var total uint64
	for _, value := range values {
		total += value
	}
	fmt.Println(total)
}
