use halo2_base::halo2_proofs::{
    circuit::{AssignedCell, Layouter, Region, SimpleFloorPlanner, Value},
    plonk::{
        Advice, Assigned, Circuit, Column, ConstraintSystem, Constraints, Error, Expression,
        Instance, Selector,
    },
    poly::Rotation,
};
use halo2_base::{
    gates::{flex_gate::FlexGateConfig, range::RangeConfig, GateInstructions, RangeInstructions},
    utils::{bigint_to_fe, biguint_to_fe, fe_to_biguint, modulus, PrimeField},
    AssignedValue, Context, QuantumCell,
};
use std::marker::PhantomData;

use crate::table::TransitionTableConfig;

// Checks a regex of string len
const STRING_LEN: usize = 22;

#[derive(Debug, Clone)]
struct RangeConstrained<F: PrimeField>(AssignedCell<F, F>);

#[derive(Debug, Clone)]
pub struct AssignedRegexResult<F: PrimeField> {
    pub characters: Vec<AssignedCell<F, F>>,
    pub states: Vec<AssignedCell<F, F>>,
}

// Here we decompose a transition into 3-value lookups.

#[derive(Debug, Clone)]
pub struct RegexCheckConfig<F: PrimeField> {
    characters: Column<Advice>,
    // characters_advice: Column<Instance>,
    state: Column<Advice>,
    transition_table: TransitionTableConfig<F>,
    q_lookup_state_selector: Selector,
    _marker: PhantomData<F>,
}

impl<F: PrimeField> RegexCheckConfig<F> {
    pub fn configure(meta: &mut ConstraintSystem<F>) -> Self {
        let characters = meta.advice_column();
        let state = meta.advice_column();
        let q_lookup_state_selector = meta.complex_selector();
        let transition_table = TransitionTableConfig::configure(meta);

        meta.enable_equality(characters);

        // Lookup each transition value individually, not paying attention to bit count
        meta.lookup("lookup characters and their state", |meta| {
            let q = meta.query_selector(q_lookup_state_selector);
            let prev_state = meta.query_advice(state, Rotation::cur());
            let next_state = meta.query_advice(state, Rotation::next());
            let character = meta.query_advice(characters, Rotation::cur());

            // One minus q
            let one_minus_q = Expression::Constant(F::from(1)) - q.clone();
            let zero = Expression::Constant(F::from(0));

            /*
                | q | state | characters | table.prev_state | table.next_state  | table.character
                | 1 | s_cur |    char    |       s_cur      |     s_next        |     char
                |   | s_next|
            */

            vec![
                (
                    q.clone() * prev_state + one_minus_q.clone() * zero.clone(),
                    transition_table.prev_state,
                ),
                (
                    q.clone() * next_state + one_minus_q.clone() * zero.clone(),
                    transition_table.next_state,
                ),
                (
                    q.clone() * character + one_minus_q.clone() * zero.clone(),
                    transition_table.character,
                ),
            ]
        });

        Self {
            characters,
            state,
            q_lookup_state_selector,
            transition_table,
            _marker: PhantomData,
        }
    }

    // Note that the two types of region.assign_advice calls happen together so that it is the same region
    pub fn assign_values(
        &self,
        region: &mut Region<F>,
        characters: &[u8],
        states: &[u64],
    ) -> Result<AssignedRegexResult<F>, Error> {
        let mut assigned_characters = Vec::new();
        let mut assigned_states = Vec::new();
        // layouter.assign_region(
        //     || "Assign values",
        //     |mut region| {
        //         // let offset = 0;

        //         // Enable q_decomposed
        //         for i in 0..STRING_LEN {
        //             println!("{:?}, {:?}", characters[i], states[i]);
        //             // offset = i;
        //             if i < STRING_LEN - 1 {
        //                 self.q_lookup_state_selector.enable(&mut region, i)?;
        //             }
        //             let assigned_c = region.assign_advice(
        //                 || format!("character"),
        //                 self.characters,
        //                 i,
        //                 || Value::known(F::from(characters[i] as u64)),
        //             )?;
        //             assigned_characters.push(assigned_c);
        //             let assigned_s = region.assign_advice(
        //                 || format!("state"),
        //                 self.state,
        //                 i,
        //                 || Value::known(F::from_u128(states[i])),
        //             )?;
        //             assigned_states.push(assigned_s);
        //         }
        //         Ok(())
        //     },
        // )?;
        // Enable q_decomposed
        for i in 0..STRING_LEN {
            println!("{:?}, {:?}", characters[i], states[i]);
            // offset = i;
            if i < STRING_LEN - 1 {
                self.q_lookup_state_selector.enable(region, i)?;
            }
            let assigned_c = region.assign_advice(
                || format!("character"),
                self.characters,
                i,
                || Value::known(F::from(characters[i] as u64)),
            )?;
            assigned_characters.push(assigned_c);
            let assigned_s = region.assign_advice(
                || format!("state"),
                self.state,
                i,
                || Value::known(F::from(states[i])),
            )?;
            assigned_states.push(assigned_s);
        }
        Ok(AssignedRegexResult {
            characters: assigned_characters,
            states: assigned_states,
        })
    }
}
#[derive(Default, Clone, Debug)]
struct RegexCheckCircuit<F: PrimeField> {
    // Since this is only relevant for the witness, we can opt to make this whatever convenient type we want
    pub characters: Vec<u8>,
    // pub states: Vec<u64>,
    _marker: PhantomData<F>,
}

impl<F: PrimeField> Circuit<F> for RegexCheckCircuit<F> {
    type Config = RegexCheckConfig<F>;
    type FloorPlanner = SimpleFloorPlanner;

    // Circuit without witnesses, called only during key generation
    fn without_witnesses(&self) -> Self {
        Self {
            characters: vec![],
            // states: vec![],
            _marker: PhantomData,
        }
    }

    fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
        let config = RegexCheckConfig::configure(meta);
        config
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        config.transition_table.load(&mut layouter)?;
        // Generate states for given characters

        // Construct transition table
        let mut array: Vec<Vec<i32>> = config
            .transition_table
            .read_2d_array::<i32>("./src/halo2_regex_lookup_body.txt");

        // Starting state is 1 always
        const START_STATE: u64 = 1u64;
        let mut states = vec![START_STATE; STRING_LEN];

        states[0] = START_STATE;
        let mut next_state = START_STATE;

        // Set the states to transition via the character and state that appear in the array, to the third value in each array tuple
        for i in 0..STRING_LEN {
            let character = self.characters[i];
            states[i] = next_state;
            let state = states[i];
            next_state = START_STATE; // Default to start state if no match found
            for j in 0..array.len() {
                if array[j][2] == character as i32 && array[j][0] == state as i32 {
                    next_state = array[j][1] as u64;
                }
            }
        }

        print!("Synthesize being called...");
        layouter.assign_region(
            || "regex",
            |mut region| {
                config.assign_values(&mut region, &self.characters, &states)?;
                Ok(())
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use halo2_base::halo2_proofs::{
        circuit::floor_planner::V1,
        dev::{CircuitCost, FailureLocation, MockProver, VerifyFailure},
        halo2curves::bn256::{Fr, G1},
        plonk::{Any, Circuit},
    };

    use super::*;

    #[test]
    fn test_regex_pass() {
        let k = 7; // 8, 128, etc

        // Convert query string to u128s
        let characters: Vec<u8> = "email was meant for @y".chars().map(|c| c as u8).collect();

        // Make a vector of the numbers 1...24
        // let states = (1..=STRING_LEN as u128).collect::<Vec<u128>>();
        assert_eq!(characters.len(), STRING_LEN);
        // assert_eq!(states.len(), STRING_LEN);

        // Successful cases
        let circuit = RegexCheckCircuit::<Fr> {
            characters,
            _marker: PhantomData,
        };

        let prover = MockProver::run(k, &circuit, vec![]).unwrap();
        prover.assert_satisfied();
        // CircuitCost::<Eq, RegexCheckCircuit<Fp>>::measure((k as u128).try_into().unwrap(), &circuit)
        println!(
            "{:?}",
            CircuitCost::<G1, RegexCheckCircuit<Fr>>::measure(
                (k as u128).try_into().unwrap(),
                &circuit
            )
        );
    }

    #[test]
    fn test_regex_fail() {
        let k = 10;

        // Convert query string to u128s
        let characters: Vec<u8> = "email isnt meant for u".chars().map(|c| c as u8).collect();

        assert_eq!(characters.len(), STRING_LEN);

        // Out-of-range `value = 8`
        let circuit = RegexCheckCircuit::<Fr> {
            characters: characters,
            // states: states,
            _marker: PhantomData,
        };
        let prover = MockProver::run(k, &circuit, vec![]).unwrap();
        match prover.verify() {
            Err(e) => {
                println!("Error successfully achieved!");
            }
            _ => assert_eq!(1, 0),
        }
    }

    // $ cargo test --release --all-features print_range_check_1
    #[cfg(feature = "dev-graph")]
    #[test]
    fn print_range_check_1() {
        use plotters::prelude::*;

        let root = BitMapBackend::new("range-check-decomposed-layout.png", (1024, 3096))
            .into_drawing_area();
        root.fill(&WHITE).unwrap();
        let root = root
            .titled("Range Check 1 Layout", ("sans-serif", 60))
            .unwrap();

        let circuit = RegexCheckCircuit::<Fp> {
            value: 2 as u128,
            _marker: PhantomData,
        };
        halo2_proofs::dev::CircuitLayout::default()
            .render(3, &circuit, &root)
            .unwrap();
    }
}
