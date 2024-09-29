   /// **hello-world** 
//    ┗━━━━━━━━━━━━━━━┹─ markdown markdown-inline
   /// **foo
//    ┗━━━━┹─ markdown markdown-inline
   fn foo() {
       println!("hello world")
//             ┗━━━━━━━━━━━━━┹─ rust
   }
   /// bar**
//    ┡━━━━┛╰─ markdown
//    ╰─ markdown markdown-inline
   fn bar() {
       println!("hello world")
//             ┗━━━━━━━━━━━━━┹─ rust
   }

