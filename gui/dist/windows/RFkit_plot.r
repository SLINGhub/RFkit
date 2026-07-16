# Configuration -------------------------------------------------------------

misc_dir <- "misc"
plot_files <- Sys.glob(file.path(misc_dir, "plot_*.bin"))

plot_colors <- c(
  "#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f",
  "#edc948", "#b07aa1", "#ff9da7", "#9c755f", "#bab0ac"
)
plot_colors_alpha20 <- paste0(plot_colors, "33")
plot_colors_alpha50 <- paste0(plot_colors, "80")


# Plot files ----------------------------------------------------------------

.PlotRfkitFile <- function(plot_file) {
  pdf_file <- basename(paste0(
    substr(plot_file, 1, nchar(plot_file) - 8),
    "pdf"
  ))

  pdf(pdf_file, paper = "a4r", width = 0, height = 0)
  on.exit(dev.off(), add = TRUE)

  par(
    mfrow = c(5, 2),
    oma = c(0, 0, 0, 0),
    mar = c(2, 2, 1.5, 0)
  )

  print(pdf_file)

  plot_connection <- file(plot_file, "rb")
  on.exit(close(plot_connection), add = TRUE)

  n_compounds <- readBin(
    plot_connection,
    integer(),
    size = 1,
    signed = FALSE,
    endian = "little"
  )

  for (compound_index in seq_len(n_compounds)) {
    color_index <- ((compound_index - 1) %% length(plot_colors_alpha50)) + 1

    compound_name <- readBin(plot_connection, character())
    xic_length <- readBin(
      plot_connection,
      integer(),
      size = 2,
      signed = FALSE,
      endian = "little"
    )
    xic_matrix <- matrix(
      readBin(
        plot_connection,
        numeric(),
        size = 4,
        n = xic_length * 2,
        endian = "little"
      ),
      ncol = xic_length
    )
    rt_xic <- xic_matrix[1, ]
    intensity_xic <- xic_matrix[2, ]

    n_wells <- readBin(
      plot_connection,
      integer(),
      size = 1,
      signed = FALSE,
      endian = "little"
    )

    for (well_index in seq_len(n_wells)) {
      well_label <- readBin(plot_connection, character())
      rt_left <- readBin(
        plot_connection,
        integer(),
        size = 2,
        signed = FALSE,
        endian = "little"
      )
      rt_right <- readBin(
        plot_connection,
        integer(),
        size = 2,
        signed = FALSE,
        endian = "little"
      )

      rt_length <- (rt_xic[rt_right] - rt_xic[rt_left]) / 2
      rt_start <- rt_xic[rt_left] - rt_length
      rt_end <- rt_xic[rt_right] + rt_length

      plot_left <- findInterval(rt_start, rt_xic)
      plot_right <- findInterval(rt_end, rt_xic)

      plot(
        x = rt_xic[plot_left:plot_right],
        y = intensity_xic[plot_left:plot_right],
        pch = ".",
        cex = 4,
        main = paste(compound_name, well_label),
        col.main = plot_colors[color_index],
        xlab = "",
        ylab = "",
        ylim = c(0, max(intensity_xic[rt_left:rt_right])),
        yaxt = "n"
      )

      y_axis_ticks <- axTicks(2)
      axis(2, at = c(0, y_axis_ticks[length(y_axis_ticks) - 1]))

      rect(
        rt_xic[rt_left],
        par("usr")[3],
        rt_xic[rt_right],
        par("usr")[4],
        col = plot_colors_alpha20[color_index],
        border = NA
      )

      polygon(
        x = c(rt_xic[rt_left:rt_right], rt_xic[rt_right], rt_xic[rt_left]),
        y = c(intensity_xic[rt_left:rt_right], 0, 0),
        col = plot_colors_alpha50[color_index],
        border = NA
      )
    }
  }

  return(invisible(pdf_file))
}


# Main ----------------------------------------------------------------------

pdf_files <- lapply(plot_files, .PlotRfkitFile)
