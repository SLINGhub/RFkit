# RFkit
For compiled executables, please go to [Releases](https://github.com/SLINGhub/RFkit/releases/tag/RF) page. 

RFkit is a freely distributed software package for processing targeted metabolomics data from RapidFire-MS experiments.

## What it does

The tool takes the following as input: (i) mzML files containing the data for individual plates, (ii) transition information and (iii) batch log file containing sipping time information from the RapidFire system. The package automatically splices the data into individual injections and reports the total ion intensities of transitions integrated over the analysis time, with edges trimmed to yield expected peak shapes.


## Building from source

Once the repository is downloaded:

```bash
cd RFkit
cargo build --release
```
