   /// **hello-world** 
// ┡━━┛┡┛┡━━━━━━━━━┛┡┛╰─ comment
// │   │ │          ╰─ comment markup.bold punctuation.bracket
// │   │ ╰─ comment markup.bold
// │   ╰─ comment markup.bold punctuation.bracket
// ╰─ comment
   /// **foo
// ┡━━┛┡┛┗━┹─ comment markup.bold
// │   ╰─ comment markup.bold punctuation.bracket
// ╰─ comment
   fn foo() {
// ┡┛ ┡━┛┡┛ ╰─ punctuation.bracket
// │  │  ╰─ punctuation.bracket
// │  ╰─ function
// ╰─ keyword.function
       println!("hello world")
//     ┡━━━━━━┛╿┡━━━━━━━━━━━┛╰─ punctuation.bracket
//     │       │╰─ string
//     │       ╰─ punctuation.bracket
//     ╰─ function.macro
   }
// ╰─ punctuation.bracket
   /// bar**
// ┡━┛┡━━┛┡┛╰─ comment
// │  │   ╰─ comment markup.bold punctuation.bracket
// │  ╰─ comment markup.bold
// ╰─ comment
   fn bar() {
// ┡┛ ┡━┛┡┛ ╰─ punctuation.bracket
// │  │  ╰─ punctuation.bracket
// │  ╰─ function
// ╰─ keyword.function
       println!("hello world")
//     ┡━━━━━━┛╿┡━━━━━━━━━━━┛╰─ punctuation.bracket
//     │       │╰─ string
//     │       ╰─ punctuation.bracket
//     ╰─ function.macro
   }
// ╰─ punctuation.bracket
