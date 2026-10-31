# Hertzforge

Hertzforge will be one toolset for structural dynamics testing, from
acquiring the data to analyzing it. It is being built by merging two
existing projects: Eigen, Kevin Cross's data acquisition software, and
Visual Dynamics, Brandon Zwink's structural dynamics toolset
(https://visualdynamics.org).

This release (0.0.1) does not contain that code yet. It installs a
`hertzforge` command that prints its version and a short help message,
and nothing else. Later releases will bring the acquisition and analysis
features across.

```
pip install hertzforge
hertzforge --version
```

Hertzforge is released under the MIT License. The source is at
https://github.com/hertzforge/hertzforge.
