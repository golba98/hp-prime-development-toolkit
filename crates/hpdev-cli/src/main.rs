mod cli;
mod driver;
mod tui;
#[cfg(feature = "web")]
mod web;

use hpdev_core::{
    Event, Request,
    host::{Emit, InputState},
    protocol::{read_packet, write_packet},
};
use std::{
    cell::RefCell,
    io,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

fn worker() -> io::Result<()> {
    let request: Request = read_packet(&mut io::stdin().lock())?;
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(64);
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            match read_packet::<hpdev_core::Control>(&mut input) {
                Ok(hpdev_core::Control::Cancel) => {
                    flag.store(true, Ordering::Release);
                }
                Ok(command) => {
                    if tx.send(command).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    flag.store(true, Ordering::Release);
                    // If the supervisor disappears, its pipe closes. Even a
                    // native builtin that does not poll the flag must not be
                    // left running indefinitely in an orphaned worker.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    std::process::exit(130);
                }
            }
        }
    });
    let broken = cancel.clone();
    let emit: Emit = Rc::new(move |event| {
        if write_packet(&mut io::stdout().lock(), &event).is_err() {
            broken.store(true, Ordering::Release);
        }
    });
    let input = Rc::new(RefCell::new(InputState::new(rx, cancel)));
    let report = hpdev_core::execution::execute(request, input, emit);
    let result = write_packet(
        &mut io::stdout().lock(),
        &Event::Final {
            report: Box::new(report.clone()),
        },
    );
    if result
        .as_ref()
        .is_err_and(|e| e.to_string().contains("packet exceeds"))
    {
        let mut compact = hpdev_core::Report::new(&report.operation, report.file);
        compact.fail(
            "resource_limit",
            6,
            "result exceeds the 32 MiB worker transport limit",
        );
        compact.instructions = report.instructions;
        compact.frames_presented = report.frames_presented;
        return write_packet(
            &mut io::stdout().lock(),
            &Event::Final {
                report: Box::new(compact),
            },
        );
    }
    result
}

fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("__worker") {
        return match worker() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("hpdev worker: {e}");
                std::process::ExitCode::from(70)
            }
        };
    }
    std::process::ExitCode::from(cli::main())
}
