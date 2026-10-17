This is a work-in-progress version of a future `drift: 3.0.0` release.
For more information, see [this issue](https://github.com/simolus3/drift/issues/3461).

These sources are not exported, and there's a test ensuring that nothing in `drift`
imports or exports `drift3_preview` (and vice-versa).
By making this part of drift package sources, we can use it in `drift_dev` more easily.

This package is tested in `future/drift3`.
