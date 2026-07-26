# frozen_string_literal: true

load File.expand_path("../bin/benchmark", __dir__)

module BenchmarkAssertions
  module_function

  def assert(condition, message)
    raise message unless condition
  end

  def includes(value, expected)
    assert(value.include?(expected), "expected output to contain #{expected.inspect}")
  end
end

ids = PopBenchmark::REGISTRY.runtimes.map(&:id)
%w[ruby python javascript rust go c d csharp crystal poplang].each do |id|
  BenchmarkAssertions.assert(ids.include?(id), "benchmark matrix is missing #{id}")
end
%w[rust go c d csharp crystal poplang].each do |id|
  runtime = PopBenchmark::REGISTRY.runtimes.find { |candidate| candidate.id == id }
  BenchmarkAssertions.assert(runtime.builder, "#{id} must compile outside the timed region")
end
%w[ruby python javascript lua luajit luau luaujit].each do |id|
  runtime = PopBenchmark::REGISTRY.runtimes.find { |candidate| candidate.id == id }
  BenchmarkAssertions.assert(!runtime.builder, "#{id} must execute its script directly")
end

workloads = PopBenchmark::REGISTRY.workloads
%w[fibonacci integerLoop tableLoop allocationChurn objectArray].each do |id|
  workload = workloads.find { |candidate| candidate.id == id }
  BenchmarkAssertions.assert(workload, "benchmark matrix is missing #{id}")
  BenchmarkAssertions.assert(!workload.category.empty?, "#{id} needs a category")
  BenchmarkAssertions.assert(workload.expected_output.end_with?("\n"), "#{id} needs an exact line checksum")
end

PopBenchmark::REGISTRY.runtimes.product(workloads).each do |runtime, workload|
  source = File.join(
    PopBenchmark::DIRECTORY, "workloads", runtime.source_directory,
    "#{workload.id}.#{runtime.source_extension}"
  )
  BenchmarkAssertions.assert(File.file?(source), "benchmark matrix is missing #{source}")
end

BenchmarkAssertions.assert(File.executable?(File.expand_path("../bin/benchmark", __dir__)), "bin/benchmark must be executable")
BenchmarkAssertions.assert(PopBenchmark::Cli.respond_to?(:batch), "CLI must provide an end-to-end batch command")
%w[Dockerfile compose.yaml].each do |name|
  BenchmarkAssertions.assert(File.file?(File.expand_path("../#{name}", __dir__)), "benchmark is missing #{name}")
end

checksum_workload = workloads.find { |candidate| candidate.id == "integerLoop" }
validator = PopBenchmark::Runner.allocate
validator.send(:validate_command, [RbConfig.ruby, "-e", "puts 1250000025000000"], checksum_workload)
validator.send(:validate_command, [RbConfig.ruby, "-e", "puts '1.250000025e+15'"], checksum_workload)
begin
  validator.send(:validate_command, [RbConfig.ruby, "-e", "puts 0"], checksum_workload)
  raise "incorrect workload output was accepted"
rescue RuntimeError => error
  BenchmarkAssertions.includes(error.message, "checksum mismatch")
end

affinity_runner = PopBenchmark::Runner.new(
  samples: 1,
  warmups: 0,
  selected_runtimes: ["poplang"],
  selected_workloads: ["objectArray"],
  cpu_affinity: 2
)
BenchmarkAssertions.assert(
  affinity_runner.send(:affinity_command, ["benchmark"]) ==
    ["taskset", "--cpu-list", "2", "benchmark"],
  "declared CPU affinity must wrap every measured execution"
)

pop_runtime = PopBenchmark::REGISTRY.runtimes.find { |runtime| runtime.id == "poplang" }
allocation_churn = workloads.find { |workload| workload.id == "allocationChurn" }
pop_result = validator.send(:result, pop_runtime, allocation_churn, [0.01])
BenchmarkAssertions.assert(
  pop_result.fetch("collectorStage") == "NativeStableGenerationalConformance",
  "Pop Lang benchmark results must identify the native collector stage"
)

begin
  validator.send(
    :execute,
    [RbConfig.ruby, "-e", "puts 0"],
    allocation_churn
  )
  raise "a timed checksum mismatch was accepted"
rescue RuntimeError => error
  BenchmarkAssertions.includes(error.message, "checksum mismatch")
end

def heap_gate_document
  host = {
    "targetArchitecture" => "x86_64",
    "targetOperatingSystem" => "linux",
    "availableParallelism" => 12,
    "processorIdentity" => "test processor",
    "cpuAffinity" => "unrestricted"
  }
  results = %w[allocationChurn objectArray].map do |workload|
    expected = PopBenchmark::REGISTRY.workloads.find { |entry| entry.id == workload }.expected_output
    {
      "runtime" => "poplang",
      "collectorStage" => "NativeStableGenerationalConformance",
      "workload" => workload,
      "samplesNanoseconds" => Array.new(50, 100),
      "samplePeakRssKiB" => Array.new(50, 1_000),
      "medianNanoseconds" => 100,
      "p99Nanoseconds" => 100,
      "peakRssKiB" => 1_000,
      "workloadSourceSha256" => workload == "allocationChurn" ? "a" * 64 : "b" * 64,
      "expectedChecksumSha256" => Digest::SHA256.hexdigest(expected),
      "checksumValidated" => true
    }
  end
  {
    "schemaVersion" => 3,
    "samples" => 50,
    "timingMeasurement" => "monotonic wall clock, integer nanoseconds",
    "memoryMeasurement" => "maximum resident set, KiB",
    "hostProfile" => host,
    "results" => results
  }
end

baseline = heap_gate_document
candidate = Marshal.load(Marshal.dump(baseline))
candidate.fetch("results").each do |result|
  result["samplesNanoseconds"] = Array.new(50, 105)
  result["medianNanoseconds"] = 105
  result["p99Nanoseconds"] = 105
  result["samplePeakRssKiB"] = Array.new(50, 1_050)
  result["peakRssKiB"] = 1_050
end
gate = PopBenchmark::HeapGate.evaluate(baseline, candidate)
BenchmarkAssertions.assert(gate.fetch("passed"), "an exact 5% heap regression must pass")

candidate.fetch("results").find { |result| result.fetch("workload") == "objectArray" }.tap do |result|
  result["samplesNanoseconds"] = Array.new(50, 106)
  result["medianNanoseconds"] = 106
  result["p99Nanoseconds"] = 106
end
gate = PopBenchmark::HeapGate.evaluate(baseline, candidate)
BenchmarkAssertions.assert(!gate.fetch("passed"), "a heap regression above 5% must fail")
BenchmarkAssertions.assert(
  gate.fetch("failures").any? { |failure| failure.include?("objectArray median") },
  "the gate must identify the regressed workload and metric"
)

incomplete = Marshal.load(Marshal.dump(baseline))
incomplete["hostProfile"]["processorIdentity"] = "different processor"
incomplete["results"].reject! { |result| result.fetch("workload") == "objectArray" }
gate = PopBenchmark::HeapGate.evaluate(baseline, incomplete)
BenchmarkAssertions.assert(!gate.fetch("passed"), "incompatible incomplete evidence must fail")
BenchmarkAssertions.assert(
  gate.fetch("failures").length >= 2,
  "the heap gate must report all independent evidence failures"
)

document = {
  "schemaVersion" => 2,
  "createdAt" => "2026-07-12T12:00:00Z",
  "samples" => 3,
  "warmups" => 1,
  "timing" => "execution only",
  "workloads" => [
    { "id" => "integerLoop", "name" => "Integer loop", "category" => "CPU", "description" => "Bad </script> payload" }
  ],
  "results" => [
    {
      "runtime" => "poplang", "runtimeName" => "Pop Lang", "executionModel" => "ahead of time",
      "workload" => "integerLoop", "workloadName" => "Integer loop", "samplesSeconds" => [0.02, 0.03, 0.04],
      "medianSeconds" => 0.03, "minimumSeconds" => 0.02, "comparable" => true
    },
    {
      "runtime" => "ruby", "runtimeName" => "Ruby", "executionModel" => "interpreted",
      "workload" => "integerLoop", "workloadName" => "Integer loop", "samplesSeconds" => [0.2, 0.3, 0.4],
      "medianSeconds" => 0.3, "minimumSeconds" => 0.2, "comparable" => true
    }
  ]
}

html = PopBenchmark::Report.render(document)
[
  '<main class="dashboard">', 'id="workload"', 'id="metric"', 'id="ranking"',
  'id="distribution"', "Execution-only", "Fastest", "\\u003c/script>"
].each { |expected| BenchmarkAssertions.includes(html, expected) }
BenchmarkAssertions.includes(html, "infinite alternate")
BenchmarkAssertions.includes(html, "Math.log2")
BenchmarkAssertions.assert(!html.include?('class="ball"'), "legacy animated-ball report remains")

puts "benchmark tests: ok"
