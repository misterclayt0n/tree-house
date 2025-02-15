   fn foo(this: &Thing) {
// ┡┛ ┡━┛╿┡━━┛╿ ╿┡━━━┛╿ ╰─ punctuation.bracket
// │  │  ││   │ ││    ╰─ punctuation.bracket
// │  │  ││   │ │╰─ type
// │  │  ││   │ ╰─ keyword.storage.modifier.ref
// │  │  ││   ╰─ punctuation.delimiter
// │  │  │╰─ variable.parameter
// │  │  ╰─ punctuation.bracket
// │  ╰─ function
// ╰─ keyword.function
    this
//  ┗━━┹─ variable.parameter
   }
// ╰─ punctuation.bracket
   
   fn bar() {
// ┡┛ ┡━┛┡┛ ╰─ punctuation.bracket
// │  │  ╰─ punctuation.bracket
// │  ╰─ function
// ╰─ keyword.function
    this.foo();
//  ┡━━┛╿┡━┛┡┛╰─ punctuation.delimiter
//  │   ││  ╰─ punctuation.bracket
//  │   │╰─ function
//  │   ╰─ punctuation.delimiter
//  ╰─ variable.builtin
   }
// ╰─ punctuation.bracket
