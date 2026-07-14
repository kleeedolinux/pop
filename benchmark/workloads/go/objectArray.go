package main

import "fmt"

type item struct {
	value uint64
}

func main() {
	values := make([]*item, 200_000)
	for index := range values {
		values[index] = &item{value: uint64(index + 1)}
	}
	var total uint64
	for _, value := range values {
		total += value.value
	}
	fmt.Println(total)
}
