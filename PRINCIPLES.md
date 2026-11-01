# Core principles

These principles came out of building Visual Dynamics, whose code Hertzforge is built from, and out of building the controller it is merged with. They explain why the software looks the way it does, and they are the standard a change is measured against, whether a person is reviewing a pull request or an AI assistant is working in the repository (`AGENTS.md` points here).

Where a principle has a hard edge case, the edge case is stated. A principle that never rules anything out is not doing any work.

## What it is for

### 1. Data should be seen, and handled

A number in a table is a claim, and a plot is the evidence for it. Every measurement can be drawn, and the plots are interactive: you can zoom into a shock transient, rotate a 3-D waterfall, or drag the corner of a filter and watch the trace change under it. Where a reading has a natural 3-D form, it is drawn in 3-D, for example as channels set one behind the other along a depth axis instead of thirty curves overlaid into a band.

### 2. The workflow is the product

A structural dynamics test is a sequence of steps: acquire or import a run, mark the events, filter, integrate, compute the spectra, compare them to a specification, and write the report. The software knows that sequence. A project shows the steps a test of its type expects, filled or still empty, so the next step is on the screen and does not have to be remembered. Derived objects record what they were made from and show when their source has changed since.

### 3. Show only what applies

The interface offers what the current selection can do, and nothing else. Select a time history and the averaging, shock and filter tools appear; select a spectrum and they are gone, not grayed out. An option that cannot be used is a question the user has to answer before they can carry on, and even a grayed control costs a glance. Three rules follow from this: say a thing once, refuse invalid input when it is entered instead of reporting it afterwards, and make every table work the same way.

### 4. Anything the interface can do, code can do

The shell and the Python API are two front ends over one implementation. Every operation the shell offers runs a method that a script can call the same way: importing, computing, refreshing, generating a report, exporting. Nothing is reachable only with the mouse. This is what makes a result reproducible. An analysis done by hand on Tuesday can be replayed exactly, in a loop, over a hundred runs.

### 5. What can be read can be written

If Hertzforge imports a format, it exports that format, and the round trip is tested. A tool that can only read a format is a dead end, because the data cannot get back out of it.

The exceptions are listed here so that it is clear they are decisions and not gaps:

- A controller run file is written only by the controller. It is the record of a test as the controller ran it, so a file written from an analysis project would describe a run that never happened. The same holds for Rattlesnake `.nc4` files, which are read and never written from a project; the one thing written is a truncated copy of an existing run, with only the time data cut.
- STEP and IGES are import only. Hertzforge holds meshes, not surfaces, so writing B-rep geometry back out would mean inventing surfaces the project does not have.
- Femap `.neu` and Nastran `.pch` are read only for now, as a matter of priority. The readers exist because a correlation needs the modes; the writers wait until somebody needs to send a model back the other way.

## How it is built

### 6. Every function says what it does; the scripting surface also says how to call it

There are two standards. Everything public has a docstring and a typed signature, and the annotations are the published API, so an editor shows real types while you write a script. In addition, the scripting surface (the verbs on `Project` and the classes the package exports) has NumPy `Parameters` and `Returns` sections, because that is where somebody arrives with no context and a table of parameters is useful.

Everywhere else the docstrings are prose, on purpose. They explain the reasoning, especially where the obvious approach was tried and rejected, since several bugs were caused by code that looked right. A Parameters table cannot say that, and requiring one everywhere would replace the useful half of the documentation with the mechanical half.

### 7. Few dependencies, and boring ones

The Python dependencies are the scientific mainstream (NumPy, SciPy, pandas, pint) and the established libraries for the file formats involved. The shell is TypeScript and Svelte with no numerics of its own, and the control service is Rust. A new dependency has to be worth the cost of everyone installing it, the risk of it going unmaintained, and the limits its license may put on this project; `tools/check_licenses.py` refuses a copyleft one on every pull request. Writing a well-defined method in fifty lines is usually the better choice.

### 8. Interaction is where the time budget goes

Effort is spent where the user is waiting. Whatever is under the mouse (a dragged filter corner, a dragged averaging span, a rotating scene) updates immediately. Heavy work that nobody is watching yet is put off or combined until the gesture ends, and it never runs where it would hold up the interface. Slowness is measured before it is optimized, and the fix has nearly always been a better algorithm instead of a clever trick.

### 9. One implementation of a rule

Where two parts of the system would answer the same question, they share one answer instead of each computing its own. A formula lives in one language and one place; the shell computes nothing and draws what a service sends, and a second copy in another language exists only as a test against shared data. Plots in the shell and figures in a report draw on different surfaces but take their decisions from one place, because a picture that disagrees with the number computed beside it is worse than no picture. Duplicated code that has already drifted apart is a bug that has not been noticed yet.

### 10. AI is a welcome way to build this, and the discipline is what makes it one

Much of Visual Dynamics was written with AI assistance, and so is much of Hertzforge, on purpose. It is how two people keep a toolset this size moving and can turn a reported bug around in an afternoon. That works because of the rules around it. AI writes code that looks right, which is exactly the failure this codebase guards against, so a change is not finished when it runs. It is finished when a test fails without it (11), when the reasoning is in the docstring (6), and when someone has checked that it did not import something it should not have. The repository has a rulebook that an assistant works under. The speed is the point, and the guardrails are what make the speed safe.

### 11. Every bug becomes a test, and every test is falsified

Coverage is kept as high as real tests can make it, and the suite is kept fast enough that there is no reason not to run it. The habit matters more than the number: when a wrong answer is found, the fix comes with a test that would have caught it, pinned to the specific thing that was wrong (an axis label, a unit, a count), never to "something was built".

Each test is then checked by breaking the fix on purpose and watching the test fail. A test that still passes with the fix reverted is testing nothing, and Visual Dynamics caught several of those. An intermittent failure is treated as a bug until proved otherwise. The word "flake" is not used here, because one failure that was dismissed that way twice turned out to be a real latch both times.

### 12. Track the latest version of everything, and never pin around a break

Every dependency is declared by name with no upper bound and no pin, and the code has no branches on a dependency's version to stay compatible with an old one. When NumPy or another dependency ships a release that breaks something here, the fix is to work with the new version, not to cap the old one and move on. Lockfiles record what a build used; CI tests past them, against the latest releases.

The reasoning is about where the cost falls. A pin turns a small problem now into a large one later: caps accumulate, they conflict with each other, and eventually the project can only be installed next to very old versions of everything else. Taking each break as it arrives keeps every one of them small. It also keeps Hertzforge installable next to whatever a test engineer already has in their environment, and for a tool meant to fit into an existing workflow that decides whether it can be used at all.

The suite is what makes this affordable. It runs on a clean machine on every pull request and once a week even when nothing has changed, so a breaking change in a dependency is found by a machine ahead of time, not by a person in the middle of an analysis.

### 13. Processing is select, see, set, apply

Every computation on data follows one workflow. Select the data to process. Open the operation's panel. Set its parameters while the plot previews the effect live, on the same data it will be made from. Then run it to create the derived object. The settings are stored with the result, the preview is the evidence, and the run calls the same method a script calls, so provenance, staleness and the API work the same whichever way the computation was started. A computation with no settings is an act, not a reading: one button, with the verb at the start of its tooltip, pressed once. A new computation is not finished until it has all four parts. Settings without a preview ask the user to trust them blind, a preview without a way to apply it leaves the user nothing to do with it, and settings kept somewhere other than the operation split one task across two places.

## What these add up to

Hertzforge is for a test engineer who has a test to run or data to analyze and needs an answer they can defend. With it you can see what you have, follow a workflow that knows what comes next, work by hand or by script with the same results, and get your data back out in the format you brought it in. It is not a general numerical library.
