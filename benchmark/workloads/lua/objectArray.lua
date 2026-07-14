local values = {}
for index = 1, 200000 do values[index] = { value = index } end
local total = 0
for index = 1, #values do total = total + values[index].value end
print(total)
