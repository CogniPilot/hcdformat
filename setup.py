# ament_python (colcon) builds this package by invoking setup.py. All metadata, packages, entry
# points, and data files live in pyproject.toml; this shim just hands off to setuptools.
from setuptools import setup

setup()
