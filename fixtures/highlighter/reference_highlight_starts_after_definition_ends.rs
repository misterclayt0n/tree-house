   fn event(tx: &Sender, event: MyEvent) {
// ┡┛ ┡━━━┛╿┡┛╿ ╿┡━━━━┛╿ ┡━━━┛╿ ┡━━━━━┛╿ ╰─ punctuation.bracket
// │  │    ││ │ ││     │ │    │ │      ╰─ punctuation.bracket
// │  │    ││ │ ││     │ │    │ ╰─ type
// │  │    ││ │ ││     │ │    ╰─ punctuation.delimiter
// │  │    ││ │ ││     │ ╰─ variable.parameter
// │  │    ││ │ ││     ╰─ punctuation.delimiter
// │  │    ││ │ │╰─ type
// │  │    ││ │ ╰─ keyword.storage.modifier.ref
// │  │    ││ ╰─ punctuation.delimiter
// │  │    │╰─ variable.parameter
// │  │    ╰─ punctuation.bracket
// │  ╰─ function
// ╰─ keyword.function
    send_blocking(tx, event);
//  ┡━━━━━━━━━━━┛╿┡┛╿ ┡━━━┛╿╰─ punctuation.delimiter
//  │            ││ │ │    ╰─ punctuation.bracket
//  │            ││ │ ╰─ variable.parameter
//  │            ││ ╰─ punctuation.delimiter
//  │            │╰─ variable.parameter
//  │            ╰─ punctuation.bracket
//  ╰─ function
   }
// ╰─ punctuation.bracket
