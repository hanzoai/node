# Hanzo HMM - Hidden Markov Model Implementation

Pure Hidden Markov Model (HMM) implementation for state detection and prediction in Hanzo Node.

## What is HMM?

A Hidden Markov Model is a statistical model where the system being modeled is assumed to be a Markov process with unobserved (hidden) states. HMMs are particularly useful for:

- **State Detection**: Identifying hidden states from observable data
- **Sequence Prediction**: Predicting future observations
- **Pattern Recognition**: Finding patterns in sequential data
- **Anomaly Detection**: Identifying unusual sequences

## Difference: HMM vs HLLM

| Aspect | HMM (This Module) | HLLM (hanzo-hllm) |
|--------|-------------------|-------------------|
| **Purpose** | Pure state detection & prediction | Hybrid LLM routing system |
| **Components** | States, observations, probabilities | HMM + Hamiltonian + LLM routing |
| **Use Case** | General sequence modeling | AI model selection & pricing |
| **Complexity** | Simple, focused | Complex, multi-component |
| **Dependencies** | None (standalone) | Uses HMM internally |

### HMM (This Module)
- Classic Hidden Markov Model
- Discrete states and observations
- Viterbi algorithm for state detection
- Forward-backward algorithm
- Baum-Welch for learning

### HLLM (Hamiltonian Hidden-Markov LLM)
- Combines multiple systems:
  - HMM for regime detection (uses this module)
  - Hamiltonian mechanics for price dynamics
  - LLM routing based on detected regimes
  - BitDelta quantization for adaptation

## Features

- **Viterbi Algorithm**: Find most likely state sequence
- **Forward Algorithm**: Calculate observation probability
- **Backward Algorithm**: Compute backward probabilities
- **Baum-Welch Algorithm**: Learn parameters from data
- **Sequence Generation**: Generate observations from model

## Usage

```rust
use hanzo_hmm::HiddenMarkovModel;

// Example: Detecting fair vs loaded dice
let states = vec!["Fair", "Loaded"];
let observations = vec![1, 2, 3, 4, 5, 6];

// Initial probabilities: 50% fair, 50% loaded
let initial = vec![0.5, 0.5];

// Transition probabilities
let transitions = vec![
    vec![0.7, 0.3],  // Fair -> Fair (70%), Fair -> Loaded (30%)
    vec![0.4, 0.6],  // Loaded -> Fair (40%), Loaded -> Loaded (60%)
];

// Emission probabilities
let emissions = vec![
    vec![1.0/6.0; 6],                    // Fair die: uniform
    vec![0.1, 0.1, 0.1, 0.1, 0.1, 0.5], // Loaded die: biased to 6
];

let hmm = HiddenMarkovModel::new(
    states, observations, initial, transitions, emissions
)?;

// Detect states from observations
let observed = vec![6, 6, 6, 1, 2];
let states = hmm.viterbi(&observed)?;
// Result: ["Loaded", "Loaded", "Loaded", "Fair", "Fair"]

// Calculate probability of sequence
let probability = hmm.forward(&observed)?;

// Generate sequences
let sequence = hmm.generate(100);
```

## Algorithms

### Viterbi Algorithm
Finds the most likely sequence of hidden states given observations.

```rust
let states = hmm.viterbi(&observations)?;
```

### Forward Algorithm
Computes the probability of an observation sequence.

```rust
let prob = hmm.forward(&observations)?;
```

### Backward Algorithm
Computes backward probabilities for each state.

```rust
let beta = hmm.backward(&observations)?;
```

### Baum-Welch Algorithm
Learns HMM parameters from observation sequences.

```rust
hmm.baum_welch(&training_sequences, max_iterations, tolerance)?;
```

## Real-World Applications

### In Hanzo Node
- **Regime Detection**: Identify market regimes (bull/bear/sideways)
- **User Behavior**: Model user interaction patterns
- **System States**: Detect system health states
- **Workload Patterns**: Identify workload types

### General Applications
- **Speech Recognition**: Phoneme detection
- **Natural Language**: Part-of-speech tagging
- **Bioinformatics**: Gene sequence analysis
- **Finance**: Market regime detection

## Integration with HLLM

When used within HLLM, this HMM module provides the regime detection layer:

```rust
// In HLLM
let hmm = hanzo_hmm::HiddenMarkovModel::new(
    vec![Regime::Exploration, Regime::Exploitation, Regime::Crisis],
    observations,
    initial_probs,
    transition_matrix,
    emission_matrix,
)?;

// Detect current regime
let regime = hmm.viterbi(&recent_observations)?;

// HLLM then uses this regime for:
// - Hamiltonian price dynamics
// - LLM model selection
// - BitDelta adapter switching
```

## License

Apache 2.0